export const CONTINUE_PROMPT = "continue";

export type CompanionSessionStatus = "idle" | "running" | "stopped" | "error";

export type CompanionSession = {
  id: string;
  status: CompanionSessionStatus;
  activeTurnId?: string | null;
  preview?: string;
  name?: string | null;
  cwd?: string | null;
  updatedAt?: number | null;
};

export type StartedTurn = {
  turnId: string;
  session?: CompanionSession;
};

export type ContinueResult =
  | { status: "disabled" }
  | { status: "sent"; threadId: string; turnId: string };

export type ContinueSelection = {
  getSelectedSession(): CompanionSession | null;
  updateSession(session: CompanionSession): void;
};

export type ContinueSessionClient = {
  refreshSession(threadId: string): Promise<CompanionSession>;
  resumeSession(threadId: string): Promise<CompanionSession>;
  interruptTurn(threadId: string, turnId: string): Promise<void>;
  waitForTurnTerminal(threadId: string, turnId: string): Promise<CompanionSession | null>;
  startTurn(threadId: string, prompt: string): Promise<StartedTurn>;
};

export class ContinueController {
  #inFlight: Promise<ContinueResult> | null = null;
  readonly #selection: ContinueSelection;
  readonly #client: ContinueSessionClient;

  constructor(selection: ContinueSelection, client: ContinueSessionClient) {
    this.#selection = selection;
    this.#client = client;
  }

  canContinue(): boolean {
    return this.#inFlight === null && this.#selection.getSelectedSession() !== null;
  }

  continueSelected(): Promise<ContinueResult> {
    if (this.#inFlight !== null) {
      return this.#inFlight;
    }

    this.#inFlight = this.#continueSelected().finally(() => {
      this.#inFlight = null;
    });

    return this.#inFlight;
  }

  async #continueSelected(): Promise<ContinueResult> {
    let session = this.#selection.getSelectedSession();
    if (session === null) {
      return { status: "disabled" };
    }

    session = await this.#refreshBestEffort(session);
    this.#selection.updateSession(session);

    if (session.status === "stopped") {
      session = await this.#client.resumeSession(session.id);
      this.#selection.updateSession(session);
    }

    if (session.status === "running") {
      const activeTurnId = session.activeTurnId;
      if (activeTurnId === undefined || activeTurnId === null || activeTurnId === "") {
        throw new Error(`Cannot interrupt running session ${session.id}: active turn id is unknown.`);
      }

      let interruptError: unknown = null;
      try {
        await this.#client.interruptTurn(session.id, activeTurnId);
      } catch (error) {
        interruptError = error;
      }

      try {
        const terminalSession = await this.#client.waitForTurnTerminal(session.id, activeTurnId);
        if (terminalSession !== null) {
          session = terminalSession;
          this.#selection.updateSession(session);
        }
      } catch (waitError) {
        throw interruptError ?? waitError;
      }
    }

    const started = await this.#client.startTurn(session.id, CONTINUE_PROMPT);
    const nextSession =
      started.session ?? ({
        ...session,
        status: "running",
        activeTurnId: started.turnId,
      } satisfies CompanionSession);
    this.#selection.updateSession(nextSession);

    return {
      status: "sent",
      threadId: session.id,
      turnId: started.turnId,
    };
  }

  async #refreshBestEffort(session: CompanionSession): Promise<CompanionSession> {
    try {
      return await this.#client.refreshSession(session.id);
    } catch {
      return session;
    }
  }
}
