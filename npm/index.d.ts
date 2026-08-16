export interface TaurilessBatch {
  messages: unknown[];
}

export declare class Tauriless {
  constructor();
  send(request: string | object): void;
  run(timeoutMs?: number): TaurilessBatch;
  close(): void;
  [Symbol.dispose](): void;
}
