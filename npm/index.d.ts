export interface TaurilessBatch {
  messages: unknown[];
}

export declare const nativeLibraryPath: string;

export declare class Tauriless {
  constructor();
  send(request: string | object): void;
  drain(): TaurilessBatch;
  start(onMessage: (message: unknown) => void, interval?: number): () => void;
  close(): void;
  [Symbol.dispose](): void;
}

export declare function createTauriless(): Tauriless;
