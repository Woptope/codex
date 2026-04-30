import { createReadStream } from "node:fs";
import { stat } from "node:fs/promises";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "./build.mjs";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const port = Number.parseInt(process.env.PORT ?? "4173", 10);
let appServer;

await build();

const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", `http://${request.headers.host ?? "localhost"}`);
  if (url.pathname.startsWith("/api/")) {
    await handleApiRequest(request, response, url);
    return;
  }

  const pathname = url.pathname === "/" ? "/index.html" : url.pathname;
  const filePath = path.resolve(root, `.${decodeURIComponent(pathname)}`);

  if (!filePath.startsWith(root)) {
    response.writeHead(403);
    response.end("Forbidden");
    return;
  }

  try {
    const file = await stat(filePath);
    if (!file.isFile()) {
      response.writeHead(404);
      response.end("Not found");
      return;
    }
  } catch {
    response.writeHead(404);
    response.end("Not found");
    return;
  }

  response.setHeader("content-type", contentType(filePath));
  createReadStream(filePath).pipe(response);
});

server.listen(port, "127.0.0.1", () => {
  console.log(`Continue Button Companion: http://127.0.0.1:${port}`);
});

async function handleApiRequest(request, response, url) {
  if (request.method === "GET" && url.pathname === "/api/events") {
    appServer.addEventStream(response);
    return;
  }

  if (request.method !== "POST") {
    writeJson(response, 405, { error: "Method not allowed" });
    return;
  }

  try {
    const body = await readJsonBody(request);
    if (url.pathname === "/api/connect") {
      await appServer.connect(String(body.url ?? ""));
      writeJson(response, 200, { result: {} });
      return;
    }

    if (url.pathname === "/api/request") {
      const result = await appServer.request(String(body.method ?? ""), body.params);
      writeJson(response, 200, { result });
      return;
    }

    if (url.pathname === "/api/notify") {
      appServer.notify(String(body.method ?? ""), body.params);
      writeJson(response, 200, { result: {} });
      return;
    }

    if (url.pathname === "/api/close") {
      appServer.close();
      writeJson(response, 200, { result: {} });
      return;
    }

    writeJson(response, 404, { error: "Not found" });
  } catch (error) {
    writeJson(response, 500, {
      error: error instanceof Error ? error.message : String(error),
    });
  }
}

function contentType(filePath) {
  switch (path.extname(filePath)) {
    case ".css":
      return "text/css; charset=utf-8";
    case ".html":
      return "text/html; charset=utf-8";
    case ".js":
      return "text/javascript; charset=utf-8";
    default:
      return "application/octet-stream";
  }
}

function readJsonBody(request) {
  return new Promise((resolve, reject) => {
    let raw = "";
    request.setEncoding("utf8");
    request.on("data", (chunk) => {
      raw += chunk;
      if (raw.length > 1024 * 1024) {
        reject(new Error("Request body too large"));
        request.destroy();
      }
    });
    request.on("end", () => {
      if (raw === "") {
        resolve({});
        return;
      }

      try {
        resolve(JSON.parse(raw));
      } catch {
        reject(new Error("Invalid JSON request body"));
      }
    });
    request.on("error", reject);
  });
}

function writeJson(response, statusCode, body) {
  response.writeHead(statusCode, {
    "content-type": "application/json; charset=utf-8",
  });
  response.end(JSON.stringify(body));
}

class AppServerProxy {
  #socket = null;
  #nextId = 1;
  #pending = new Map();
  #eventStreams = new Set();

  async connect(url) {
    if (!url.startsWith("ws://") && !url.startsWith("wss://")) {
      throw new Error("Server URL must start with ws:// or wss://");
    }

    this.close();
    const socket = new WebSocket(url);
    this.#socket = socket;
    socket.addEventListener("message", (event) => {
      this.#handleMessage(String(event.data));
    });
    socket.addEventListener("close", () => {
      this.#rejectPending(new Error("App-server websocket closed."));
    });
    socket.addEventListener("error", () => {
      this.#rejectPending(new Error("App-server websocket failed."));
    });

    await new Promise((resolve, reject) => {
      socket.addEventListener("open", resolve, { once: true });
      socket.addEventListener("error", () => reject(new Error("Unable to connect to app-server.")), {
        once: true,
      });
    });
  }

  close() {
    if (this.#socket !== null) {
      this.#socket.close();
      this.#socket = null;
    }
    this.#rejectPending(new Error("App-server connection closed."));
  }

  request(method, params) {
    const socket = this.#requireSocket();
    const id = this.#nextId++;
    const message = { id, method };
    if (params !== undefined) {
      message.params = params;
    }

    const promise = new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
    });
    socket.send(JSON.stringify(message));
    return promise;
  }

  notify(method, params) {
    const socket = this.#requireSocket();
    const message = { method };
    if (params !== undefined) {
      message.params = params;
    }
    socket.send(JSON.stringify(message));
  }

  addEventStream(response) {
    response.writeHead(200, {
      "cache-control": "no-cache",
      "connection": "keep-alive",
      "content-type": "text/event-stream; charset=utf-8",
    });
    response.write("event: ready\ndata: {}\n\n");
    this.#eventStreams.add(response);
    response.on("close", () => {
      this.#eventStreams.delete(response);
    });
  }

  #handleMessage(raw) {
    let message;
    try {
      message = JSON.parse(raw);
    } catch {
      return;
    }

    if ("id" in message) {
      const pending = this.#pending.get(message.id);
      if (pending === undefined) {
        return;
      }
      this.#pending.delete(message.id);
      if (message.error !== undefined) {
        pending.reject(new Error(message.error.message));
      } else {
        pending.resolve(message.result ?? {});
      }
      return;
    }

    this.#emitNotification(message);
  }

  #emitNotification(message) {
    const payload = JSON.stringify(message);
    for (const stream of this.#eventStreams) {
      stream.write(`event: notification\ndata: ${payload}\n\n`);
    }
  }

  #rejectPending(error) {
    for (const pending of this.#pending.values()) {
      pending.reject(error);
    }
    this.#pending.clear();
  }

  #requireSocket() {
    if (this.#socket === null || this.#socket.readyState !== WebSocket.OPEN) {
      throw new Error("App-server websocket is not connected.");
    }
    return this.#socket;
  }
}

appServer = new AppServerProxy();
