import type { v2 } from "../../../../app-server-protocol/schema/typescript/index.ts";
import type {
  CompanionSession,
  ContinueSessionClient,
  StartedTurn,
} from "./continue-controller.ts";

type JsonValue =
  | null
  | boolean
  | number
  | string
  | JsonValue[]
  | { [key: string]: JsonValue | undefined };

type JsonRpcRequest = {
  id: number;
  method: string;
  params?: JsonValue;
};

type JsonRpcNotification = {
  method: string;
  params?: JsonValue;
};

type JsonRpcResponse = {
  id: number;
  result?: JsonValue;
  error?: {
    code: number;
    message: string;
    data?: JsonValue;
  };
};

type JsonRpcMessage = JsonRpcResponse | JsonRpcNotification;

type NotificationListener = (method: string, params: JsonValue | undefined) => void;

type PendingRequest = {
  resolve(value: JsonValue): void;
  reject(error: Error): void;
};

export class JsonRpcWebSocketClient {
  #socket: WebSocket | null = null;
  #nextId = 1;
  #pending = new Map<number, PendingRequest>();
  #notificationListeners = new Set<NotificationListener>();

  get connected(): boolean {
    return this.#socket?.readyState === WebSocket.OPEN;
  }

  async connect(url: string): Promise<void> {
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

    await new Promise<void>((resolve, reject) => {
      socket.addEventListener("open", () => resolve(), { once: true });
      socket.addEventListener("error", () => reject(new Error("Unable to connect to app-server.")), {
        once: true,
      });
    });
  }

  close(): void {
    if (this.#socket !== null) {
      this.#socket.close();
      this.#socket = null;
    }
    this.#rejectPending(new Error("App-server connection closed."));
  }

  onNotification(listener: NotificationListener): () => void {
    this.#notificationListeners.add(listener);
    return () => {
      this.#notificationListeners.delete(listener);
    };
  }

  request(method: string, params?: JsonValue): Promise<JsonValue> {
    const socket = this.#requireSocket();
    const id = this.#nextId++;
    const message: JsonRpcRequest = { id, method };
    if (params !== undefined) {
      message.params = params;
    }

    const promise = new Promise<JsonValue>((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
    });
    socket.send(JSON.stringify(message));
    return promise;
  }

  notify(method: string, params?: JsonValue): void {
    const socket = this.#requireSocket();
    const message: JsonRpcNotification = { method };
    if (params !== undefined) {
      message.params = params;
    }
    socket.send(JSON.stringify(message));
  }

  #handleMessage(raw: string): void {
    let message: JsonRpcMessage;
    try {
      message = JSON.parse(raw) as JsonRpcMessage;
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

    for (const listener of this.#notificationListeners) {
      listener(message.method, message.params);
    }
  }

  #rejectPending(error: Error): void {
    for (const pending of this.#pending.values()) {
      pending.reject(error);
    }
    this.#pending.clear();
  }

  #requireSocket(): WebSocket {
    if (this.#socket === null || this.#socket.readyState !== WebSocket.OPEN) {
      throw new Error("App-server websocket is not connected.");
    }
    return this.#socket;
  }
}

export class AppServerSessionClient implements ContinueSessionClient {
  #sessions = new Map<string, CompanionSession>();
  #sessionListeners = new Set<(sessions: CompanionSession[]) => void>();
  readonly #rpc: JsonRpcWebSocketClient;

  constructor(rpc: JsonRpcWebSocketClient) {
    this.#rpc = rpc;
    this.#rpc.onNotification((method, params) => {
      this.#handleNotification(method, params);
    });
  }

  async initialize(url: string): Promise<void> {
    await this.#rpc.connect(url);
    await this.#rpc.request("initialize", {
      clientInfo: {
        name: "continue_button_companion",
        title: "Continue Button Companion",
        version: "0.1.0",
      },
    });
    this.#rpc.notify("initialized");
  }

  loadTrackedSessions(sessions: CompanionSession[]): void {
    this.#sessions.clear();
    for (const session of sessions) {
      this.#sessions.set(session.id, session);
    }
    this.#emitSessions();
  }

  listTrackedSessions(): CompanionSession[] {
    return sortSessions([...this.#sessions.values()]);
  }

  onSessionsChanged(listener: (sessions: CompanionSession[]) => void): () => void {
    this.#sessionListeners.add(listener);
    return () => {
      this.#sessionListeners.delete(listener);
    };
  }

  async startSession(params: Partial<v2.ThreadStartParams>): Promise<CompanionSession> {
    const response = (await this.#rpc.request("thread/start", params as JsonValue)) as v2.ThreadStartResponse;
    return this.#trackThread(response.thread);
  }

  async trackExistingSession(threadId: string): Promise<CompanionSession> {
    return await this.resumeSession(threadId);
  }

  async refreshAll(): Promise<CompanionSession[]> {
    const sessions = this.listTrackedSessions();
    const refreshed = await Promise.allSettled(
      sessions.map((session) => this.refreshSession(session.id)),
    );

    for (let index = 0; index < refreshed.length; index += 1) {
      const result = refreshed[index];
      if (result.status === "rejected") {
        this.#sessions.set(sessions[index].id, {
          ...sessions[index],
          status: "error",
        });
      }
    }

    this.#emitSessions();
    return this.listTrackedSessions();
  }

  async refreshSession(threadId: string): Promise<CompanionSession> {
    const response = (await this.#rpc.request("thread/read", {
      threadId,
      includeTurns: true,
    })) as v2.ThreadReadResponse;
    return this.#trackThread(response.thread);
  }

  async resumeSession(threadId: string): Promise<CompanionSession> {
    const response = (await this.#rpc.request("thread/resume", {
      threadId,
      excludeTurns: false,
    })) as v2.ThreadResumeResponse;
    return this.#trackThread(response.thread);
  }

  async interruptTurn(threadId: string, turnId: string): Promise<void> {
    await this.#rpc.request("turn/interrupt", { threadId, turnId });
  }

  async waitForTurnTerminal(threadId: string, turnId: string): Promise<CompanionSession | null> {
    const deadline = Date.now() + 120_000;

    while (Date.now() < deadline) {
      const known = this.#sessions.get(threadId);
      if (known !== undefined && known.activeTurnId !== turnId) {
        return known;
      }

      try {
        const refreshed = await this.refreshSession(threadId);
        if (refreshed.activeTurnId !== turnId || refreshed.status !== "running") {
          return refreshed;
        }
      } catch {
        // Notifications can still resolve the wait; keep polling until the timeout.
      }

      await sleep(500);
    }

    throw new Error(`Timed out waiting for turn ${turnId} to finish.`);
  }

  async startTurn(threadId: string, prompt: string): Promise<StartedTurn> {
    const response = (await this.#rpc.request("turn/start", {
      threadId,
      input: [
        {
          type: "text",
          text: prompt,
          text_elements: [],
        },
      ],
    })) as v2.TurnStartResponse;

    const existing = this.#sessions.get(threadId);
    const session =
      existing === undefined
        ? undefined
        : ({
            ...existing,
            status: "running",
            activeTurnId: response.turn.id,
          } satisfies CompanionSession);
    if (session !== undefined) {
      this.#sessions.set(threadId, session);
      this.#emitSessions();
    }

    return {
      turnId: response.turn.id,
      session,
    };
  }

  #handleNotification(method: string, params: JsonValue | undefined): void {
    if (params === undefined || params === null || typeof params !== "object" || Array.isArray(params)) {
      return;
    }

    if (method === "thread/started") {
      const thread = (params as { thread?: v2.Thread }).thread;
      if (thread !== undefined) {
        this.#trackThread(thread);
      }
      return;
    }

    if (method === "thread/status/changed") {
      const { threadId, status } = params as v2.ThreadStatusChangedNotification;
      const existing = this.#sessions.get(threadId);
      if (existing !== undefined) {
        this.#sessions.set(threadId, {
          ...existing,
          status: mapThreadStatus(status),
          activeTurnId: status.type === "active" ? existing.activeTurnId : null,
        });
        this.#emitSessions();
      }
      return;
    }

    if (method === "turn/started") {
      const { threadId, turn } = params as v2.TurnStartedNotification;
      const existing = this.#sessions.get(threadId);
      if (existing !== undefined) {
        this.#sessions.set(threadId, {
          ...existing,
          status: "running",
          activeTurnId: turn.id,
        });
        this.#emitSessions();
      }
      return;
    }

    if (method === "turn/completed") {
      const { threadId, turn } = params as v2.TurnCompletedNotification;
      const existing = this.#sessions.get(threadId);
      if (existing !== undefined) {
        this.#sessions.set(threadId, {
          ...existing,
          status: "idle",
          activeTurnId: existing.activeTurnId === turn.id ? null : existing.activeTurnId,
        });
        this.#emitSessions();
      }
    }
  }

  #trackThread(thread: v2.Thread): CompanionSession {
    const existing = this.#sessions.get(thread.id);
    const session = companionSessionFromThread(thread);
    if (
      session.status === "running" &&
      session.activeTurnId === null &&
      existing?.activeTurnId !== undefined &&
      existing.activeTurnId !== null
    ) {
      session.activeTurnId = existing.activeTurnId;
    }
    this.#sessions.set(session.id, session);
    this.#emitSessions();
    return session;
  }

  #emitSessions(): void {
    const sessions = this.listTrackedSessions();
    for (const listener of this.#sessionListeners) {
      listener(sessions);
    }
  }
}

export function companionSessionFromThread(thread: v2.Thread): CompanionSession {
  const activeTurn = [...thread.turns].reverse().find((turn) => turn.status === "inProgress");
  return {
    id: thread.id,
    status: mapThreadStatus(thread.status),
    activeTurnId: activeTurn?.id ?? null,
    preview: thread.preview,
    name: thread.name,
    cwd: thread.cwd,
    updatedAt: thread.updatedAt,
  };
}

function mapThreadStatus(status: v2.ThreadStatus): CompanionSession["status"] {
  switch (status.type) {
    case "active":
      return "running";
    case "idle":
      return "idle";
    case "notLoaded":
      return "stopped";
    case "systemError":
      return "error";
  }
}

function sortSessions(sessions: CompanionSession[]): CompanionSession[] {
  return sessions.sort((left, right) => (right.updatedAt ?? 0) - (left.updatedAt ?? 0));
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => {
    window.setTimeout(resolve, ms);
  });
}
