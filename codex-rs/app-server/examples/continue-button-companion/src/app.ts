import { AppServerSessionClient, JsonRpcWebSocketClient } from "./app-server-client.ts";
import {
  type CompanionSession,
  ContinueController,
  type ContinueSelection,
} from "./continue-controller.ts";

const STORAGE_KEY = "continue-button-companion.sessions.v1";

type StoredState = {
  serverUrl: string;
  selectedSessionId: string | null;
  sessions: CompanionSession[];
};

const defaultState: StoredState = {
  serverUrl: "ws://127.0.0.1:8765",
  selectedSessionId: null,
  sessions: [],
};

const elements = {
  serverUrl: mustElement<HTMLInputElement>("server-url"),
  connect: mustElement<HTMLButtonElement>("connect"),
  connectionStatus: mustElement<HTMLElement>("connection-status"),
  cwd: mustElement<HTMLInputElement>("cwd"),
  startSession: mustElement<HTMLButtonElement>("start-session"),
  resumeSessionId: mustElement<HTMLInputElement>("resume-session-id"),
  resumeSession: mustElement<HTMLButtonElement>("resume-session"),
  refreshSessions: mustElement<HTMLButtonElement>("refresh-sessions"),
  sessionList: mustElement<HTMLElement>("session-list"),
  selectedTitle: mustElement<HTMLElement>("selected-title"),
  selectedMeta: mustElement<HTMLElement>("selected-meta"),
  selectedStatus: mustElement<HTMLElement>("selected-status"),
  continueButton: mustElement<HTMLButtonElement>("continue-button"),
  activity: mustElement<HTMLElement>("activity"),
};

let storedState = loadState();
let selectedSessionId = storedState.selectedSessionId;

const appServer = new AppServerSessionClient(new JsonRpcWebSocketClient());
const selection: ContinueSelection = {
  getSelectedSession() {
    if (selectedSessionId === null) {
      return null;
    }
    return appServer.listTrackedSessions().find((session) => session.id === selectedSessionId) ?? null;
  },
  updateSession(session) {
    if (selectedSessionId === session.id) {
      renderSelectedSession(session);
    }
    persistState();
  },
};
const continueController = new ContinueController(selection, appServer);

elements.serverUrl.value = storedState.serverUrl;
appServer.loadTrackedSessions(storedState.sessions);
appServer.onSessionsChanged((sessions) => {
  if (selectedSessionId !== null && !sessions.some((session) => session.id === selectedSessionId)) {
    selectedSessionId = null;
  }
  render();
  persistState();
});

elements.connect.addEventListener("click", () => {
  void runAction("Connected", async () => {
    await appServer.initialize(elements.serverUrl.value.trim());
    elements.connectionStatus.textContent = "Connected";
    elements.connectionStatus.dataset.state = "connected";
    await appServer.refreshAll();
  });
});

elements.startSession.addEventListener("click", () => {
  void runAction("Session started", async () => {
    const cwd = elements.cwd.value.trim();
    const session = await appServer.startSession(cwd === "" ? {} : { cwd });
    selectedSessionId = session.id;
    render();
    persistState();
  });
});

elements.resumeSession.addEventListener("click", () => {
  void runAction("Session resumed", async () => {
    const threadId = elements.resumeSessionId.value.trim();
    if (threadId === "") {
      throw new Error("Enter a session id.");
    }
    const session = await appServer.trackExistingSession(threadId);
    selectedSessionId = session.id;
    elements.resumeSessionId.value = "";
    render();
    persistState();
  });
});

elements.refreshSessions.addEventListener("click", () => {
  void runAction("Sessions refreshed", async () => {
    await appServer.refreshAll();
  });
});

elements.continueButton.addEventListener("click", () => {
  void runAction("Continue sent", async () => {
    const result = await continueController.continueSelected();
    if (result.status === "disabled") {
      setActivity("Select a session first", "muted");
    }
    render();
  });
});

elements.serverUrl.addEventListener("change", () => {
  storedState.serverUrl = elements.serverUrl.value.trim();
  persistState();
});

render();
void autoConnect();

function render(): void {
  const sessions = appServer.listTrackedSessions();
  elements.sessionList.replaceChildren(...sessions.map(renderSessionRow));
  if (sessions.length === 0) {
    const empty = document.createElement("div");
    empty.className = "session-empty";
    empty.textContent = appServer.connected ? "No companion sessions" : "Disconnected";
    elements.sessionList.append(empty);
  }

  const selectedSession =
    selectedSessionId === null
      ? null
      : sessions.find((session) => session.id === selectedSessionId) ?? null;
  renderSelectedSession(selectedSession);
  elements.continueButton.disabled = !continueController.canContinue();
  renderConnectionControls(false);
}

function renderSessionRow(session: CompanionSession): HTMLElement {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "session-row";
  button.dataset.selected = String(session.id === selectedSessionId);
  button.addEventListener("click", () => {
    selectedSessionId = session.id;
    render();
    persistState();
  });

  const title = document.createElement("span");
  title.className = "session-title";
  title.textContent = session.name ?? session.preview ?? session.id;

  const meta = document.createElement("span");
  meta.className = "session-meta";
  meta.textContent = `${formatStatus(session.status)} · ${shortId(session.id)}`;

  button.append(title, meta);
  return button;
}

function renderSelectedSession(session: CompanionSession | null): void {
  if (session === null) {
    elements.selectedTitle.textContent = "No session selected";
    elements.selectedMeta.textContent = "";
    elements.selectedStatus.textContent = "None";
    elements.selectedStatus.dataset.state = "none";
    elements.continueButton.disabled = true;
    return;
  }

  elements.selectedTitle.textContent = session.name ?? session.preview ?? session.id;
  elements.selectedMeta.textContent = `${session.cwd ?? "No cwd"} · ${session.id}`;
  elements.selectedStatus.textContent = formatStatus(session.status);
  elements.selectedStatus.dataset.state = session.status;
  elements.continueButton.disabled = !continueController.canContinue();
}

async function runAction(successMessage: string, action: () => Promise<void>): Promise<void> {
  setControlsDisabled(true);
  setActivity("Working", "muted");
  try {
    await action();
    setActivity(successMessage, "ok");
  } catch (error) {
    setActivity(error instanceof Error ? error.message : String(error), "error");
  } finally {
    setControlsDisabled(false);
    render();
  }
}

function setControlsDisabled(disabled: boolean): void {
  elements.connect.disabled = disabled;
  renderConnectionControls(disabled);
  elements.continueButton.disabled = disabled || !continueController.canContinue();
}

function renderConnectionControls(forceDisabled: boolean): void {
  const disabled = forceDisabled || !appServer.connected;
  elements.startSession.disabled = disabled;
  elements.resumeSession.disabled = disabled;
  elements.refreshSessions.disabled = disabled;
  elements.connect.disabled = forceDisabled;
  if (appServer.connected) {
    elements.connectionStatus.textContent = "Connected";
    elements.connectionStatus.dataset.state = "connected";
  }
}

async function autoConnect(): Promise<void> {
  try {
    await appServer.initialize(elements.serverUrl.value.trim());
    elements.connectionStatus.textContent = "Connected";
    elements.connectionStatus.dataset.state = "connected";
    await appServer.refreshAll();
    setActivity("Connected", "ok");
  } catch {
    elements.connectionStatus.textContent = "Disconnected";
    elements.connectionStatus.dataset.state = "none";
    setActivity("Start app-server, then connect", "muted");
  } finally {
    render();
  }
}

function setActivity(message: string, state: "muted" | "ok" | "error"): void {
  elements.activity.textContent = message;
  elements.activity.dataset.state = state;
}

function persistState(): void {
  const state: StoredState = {
    serverUrl: elements.serverUrl.value.trim(),
    selectedSessionId,
    sessions: appServer.listTrackedSessions(),
  };
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
}

function loadState(): StoredState {
  const raw = window.localStorage.getItem(STORAGE_KEY);
  if (raw === null) {
    return defaultState;
  }

  try {
    return { ...defaultState, ...(JSON.parse(raw) as Partial<StoredState>) };
  } catch {
    return defaultState;
  }
}

function mustElement<T extends HTMLElement>(id: string): T {
  const element = document.getElementById(id);
  if (element === null) {
    throw new Error(`Missing element: ${id}`);
  }
  return element as T;
}

function shortId(id: string): string {
  return id.length <= 12 ? id : `${id.slice(0, 8)}...${id.slice(-4)}`;
}

function formatStatus(status: CompanionSession["status"]): string {
  switch (status) {
    case "idle":
      return "Idle";
    case "running":
      return "Running";
    case "stopped":
      return "Stopped";
    case "error":
      return "Error";
  }
}
