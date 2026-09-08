import type { BridgeHost, ChangeCursor, StoredRecord } from "./client";
import { readReceipt, type Receipt } from "./receipts";

export type TranscriptItem = { record: StoredRecord } & (
  | { kind: "message"; role: "user" | "agent" | "reasoning"; text: string }
  | { kind: "tool"; title: string; status: "pending" | "running" | "completed" | "failed" | "unknown" }
  | { kind: "permission"; title: string; options: { id: string; label: string; effect: string }[] }
  | { kind: "failure"; message: string }
  | { kind: "run_finished"; reason: string }
  | { kind: "receipt"; receipt: Receipt }
  | { kind: "other" }
);

// Raw records remain available, including non-text content and extension receipts.
// These readers validate the fields they expose instead of asserting unknown JSON types.
export function project(record: StoredRecord): TranscriptItem {
  const data = record.payload.data as any;
  const invalid = () => { throw new Error(`invalid ${record.payload.type} record ${record.id}`); };
  switch (record.payload.type) {
    case "extension": {
      const receipt = readReceipt(record);
      return receipt ? {kind:"receipt",receipt,record} : {kind:"other",record};
    }
    case "message": {
      if (!data || !["user", "agent", "reasoning"].includes(data.kind) || !Array.isArray(data.message?.content)) return invalid();
      const text = data.message.content.filter((part: any) => part.type === "text").map((part: any) => {
        if (typeof part.data !== "string") return invalid(); return part.data;
      }).join("");
      return { kind: "message", role: data.kind, text, record };
    }
    case "tool":
      if (typeof data?.title !== "string" || !["pending", "running", "completed", "failed", "unknown"].includes(data.status)) return invalid();
      return { kind: "tool", title: data.title, status: data.status, record };
    case "permission":
      if (typeof data?.title !== "string" || !Array.isArray(data.options) || !data.options.every((o: any) => typeof o.id === "string" && typeof o.label === "string" && typeof o.effect === "string")) return invalid();
      return { kind: "permission", title: data.title, options: data.options, record };
    case "failure":
      if (typeof data?.message !== "string") return invalid();
      return { kind: "failure", message: data.message, record };
    case "run_finished":
      if (typeof data?.reason?.type !== "string") return invalid();
      return { kind: "run_finished", reason: data.reason.type, record };
    default: return { kind: "other", record };
  }
}

/** Latest-record projection. Polling is explicit; no unbounded background subscriber. */
export interface StateCheckpoint { projection_version?: number; cursor: ChangeCursor; records: StoredRecord[] }
export class SessionState {
  private records = new Map<string, StoredRecord>();
  private position?: ChangeCursor;
  private syncing = false;
  get cursor() { return this.position && { ...this.position }; }
  get items(): TranscriptItem[] {
    return [...this.records.values()].sort((a, b) => BigInt(a.sequence) < BigInt(b.sequence) ? -1 : BigInt(a.sequence) > BigInt(b.sequence) ? 1 : 0).map(project);
  }
  constructor(readonly database: string, readonly sessionId: string, checkpoint?: StateCheckpoint) {
    if (checkpoint) {
      if (checkpoint.cursor.session_id !== sessionId || checkpoint.records.some(row => row.session_id !== sessionId)) throw new Error("foreign state checkpoint");
      // Legacy checkpoints have no receipt metadata. Rescan instead of keeping
      // same-revision records forever without the new projection fields.
      if (checkpoint.projection_version === undefined) return;
      if (checkpoint.projection_version !== 1) throw new Error("unsupported state projection version");
      const copy = structuredClone(checkpoint);
      copy.records.forEach(project);
      this.records = new Map(copy.records.map(row => [row.id, row])); this.position = copy.cursor;
    }
  }
  checkpoint(): StateCheckpoint | undefined {
    return this.position && structuredClone({ projection_version:1, cursor: this.position, records: [...this.records.values()] });
  }
  async sync(host: BridgeHost, limit = 100) {
    if (this.syncing) throw new Error("state sync already in progress");
    this.syncing = true;
    // Commit projection and cursor together only after every page succeeds.
    const records = new Map(this.records);
    let cursor = this.position;
    let pages = 0;
    const budget = () => { if (++pages > 1000) throw new Error("state sync page budget exceeded; no state was committed"); };
    const apply = (rows: StoredRecord[]) => {
      for (const row of rows) {
        if (row.session_id !== this.sessionId) throw new Error("foreign session record");
        project(row);
        const old = records.get(row.id);
        if (!old || BigInt(row.revision) > BigInt(old.revision)) records.set(row.id, row);
      }
    };
    try {
      if (!cursor) {
        let after: string | undefined;
        do {
          budget();
          const page = await host.snapshot(this.database, this.sessionId, cursor, after, limit);
          cursor ??= page.cursor;
          apply(page.records);
          if (!page.page_full) break;
          after = page.next_after!;
        } while (true);
      }
      do {
        budget();
        const page = await host.changes(this.database, this.sessionId, cursor!, limit);
        apply(page.records); cursor = page.cursor;
        if (!page.page_full) break;
      } while (true);
      this.records = records; this.position = cursor;
    } finally { this.syncing = false; }
  }
}
