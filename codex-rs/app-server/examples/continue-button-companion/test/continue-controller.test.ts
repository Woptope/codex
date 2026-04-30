import assert from "node:assert/strict";
import { describe, it } from "node:test";
import {
  CONTINUE_PROMPT,
  type CompanionSession,
  ContinueController,
  type ContinueSelection,
  type ContinueSessionClient,
  type StartedTurn,
} from "../src/continue-controller.ts";

class FakeSelection implements ContinueSelection {
  selected: CompanionSession | null;

  constructor(selected: CompanionSession | null) {
    this.selected = selected;
  }

  updates: CompanionSession[] = [];

  getSelectedSession(): CompanionSession | null {
    return this.selected;
  }

  updateSession(session: CompanionSession): void {
    this.selected = session;
    this.updates.push(session);
  }
}

class FakeClient implements ContinueSessionClient {
  calls: string[] = [];
  refreshSessionResult: CompanionSession | null = null;
  resumeSessionResult: CompanionSession | null = null;
  waitForTurnTerminalResult: CompanionSession | null = null;
  startTurnDelay: Promise<void> | null = null;
  interruptTurnImpl: ((threadId: string, turnId: string) => Promise<void>) | null = null;

  async refreshSession(threadId: string): Promise<CompanionSession> {
    this.calls.push(`refresh:${threadId}`);
    if (this.refreshSessionResult === null) {
      throw new Error("refresh unavailable");
    }
    return this.refreshSessionResult;
  }

  async resumeSession(threadId: string): Promise<CompanionSession> {
    this.calls.push(`resume:${threadId}`);
    if (this.resumeSessionResult === null) {
      throw new Error("resume unavailable");
    }
    return this.resumeSessionResult;
  }

  async interruptTurn(threadId: string, turnId: string): Promise<void> {
    this.calls.push(`interrupt:${threadId}:${turnId}`);
    if (this.interruptTurnImpl !== null) {
      await this.interruptTurnImpl(threadId, turnId);
    }
  }

  async waitForTurnTerminal(threadId: string, turnId: string): Promise<CompanionSession | null> {
    this.calls.push(`wait:${threadId}:${turnId}`);
    return this.waitForTurnTerminalResult;
  }

  async startTurn(threadId: string, prompt: string): Promise<StartedTurn> {
    this.calls.push(`start:${threadId}:${prompt}`);
    if (this.startTurnDelay !== null) {
      await this.startTurnDelay;
    }
    return { turnId: "turn-continue" };
  }
}

describe("ContinueController", () => {
  it("sends exactly one continue prompt for an idle selected session", async () => {
    const session: CompanionSession = { id: "thread-1", status: "idle" };
    const selection = new FakeSelection(session);
    const client = new FakeClient();
    client.refreshSessionResult = session;

    const controller = new ContinueController(selection, client);
    const result = await controller.continueSelected();

    assert.deepEqual(result, {
      status: "sent",
      threadId: "thread-1",
      turnId: "turn-continue",
    });
    assert.equal(client.calls.filter((call) => call === `start:thread-1:${CONTINUE_PROMPT}`).length, 1);
  });

  it("interrupts a running selected session, waits, then sends one continue", async () => {
    const running: CompanionSession = {
      id: "thread-1",
      status: "running",
      activeTurnId: "turn-active",
    };
    const idle: CompanionSession = { id: "thread-1", status: "idle", activeTurnId: null };
    const selection = new FakeSelection(running);
    const client = new FakeClient();
    client.refreshSessionResult = running;
    client.waitForTurnTerminalResult = idle;

    const controller = new ContinueController(selection, client);
    await controller.continueSelected();

    assert.deepEqual(client.calls, [
      "refresh:thread-1",
      "interrupt:thread-1:turn-active",
      "wait:thread-1:turn-active",
      `start:thread-1:${CONTINUE_PROMPT}`,
    ]);
  });

  it("still sends continue when the turn finishes before interrupt resolves", async () => {
    const running: CompanionSession = {
      id: "thread-1",
      status: "running",
      activeTurnId: "turn-active",
    };
    const idle: CompanionSession = { id: "thread-1", status: "idle", activeTurnId: null };
    const selection = new FakeSelection(running);
    const client = new FakeClient();
    client.refreshSessionResult = running;
    client.waitForTurnTerminalResult = idle;
    client.interruptTurnImpl = async () => {
      throw new Error("turn is already completed");
    };

    const controller = new ContinueController(selection, client);
    await controller.continueSelected();

    assert.equal(client.calls.filter((call) => call === `start:thread-1:${CONTINUE_PROMPT}`).length, 1);
  });

  it("coalesces double-clicks into a single continue prompt", async () => {
    const session: CompanionSession = { id: "thread-1", status: "idle" };
    const selection = new FakeSelection(session);
    const client = new FakeClient();
    client.refreshSessionResult = session;
    let releaseStart!: () => void;
    client.startTurnDelay = new Promise<void>((resolve) => {
      releaseStart = resolve;
    });

    const controller = new ContinueController(selection, client);
    const first = controller.continueSelected();
    const second = controller.continueSelected();
    releaseStart();

    await Promise.all([first, second]);

    assert.equal(client.calls.filter((call) => call === `start:thread-1:${CONTINUE_PROMPT}`).length, 1);
  });

  it("disables continue when no session is selected", async () => {
    const selection = new FakeSelection(null);
    const client = new FakeClient();
    const controller = new ContinueController(selection, client);

    assert.equal(controller.canContinue(), false);
    assert.deepEqual(await controller.continueSelected(), { status: "disabled" });
    assert.deepEqual(client.calls, []);
  });
});
