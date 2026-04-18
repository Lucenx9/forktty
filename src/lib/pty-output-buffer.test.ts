import { describe, expect, it } from "vitest";
import {
  createPtyOutputBufferState,
  enqueueBoundedPtyOutput,
  takePtyOutputBatch,
} from "./pty-output-buffer";

function bytes(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}

function text(data: Uint8Array | null): string {
  if (!data) return "";
  return new TextDecoder().decode(data);
}

describe("pty-output-buffer", () => {
  it("drops the oldest buffered output when the cap is exceeded", () => {
    const state = createPtyOutputBufferState();

    expect(enqueueBoundedPtyOutput(state, bytes("abcd"), 6)).toBe(0);
    expect(enqueueBoundedPtyOutput(state, bytes("efgh"), 6)).toBe(2);

    expect(state.totalBytes).toBe(6);
    expect(text(takePtyOutputBatch(state, 6))).toBe("cdefgh");
  });

  it("keeps the tail of a single oversized chunk", () => {
    const state = createPtyOutputBufferState();

    const dropped = enqueueBoundedPtyOutput(state, bytes("0123456789"), 4);

    expect(dropped).toBe(6);
    expect(state.totalBytes).toBe(4);
    expect(text(takePtyOutputBatch(state, 4))).toBe("6789");
  });

  it("returns batches without losing the remaining queue", () => {
    const state = createPtyOutputBufferState();

    enqueueBoundedPtyOutput(state, bytes("abcd"), 16);
    enqueueBoundedPtyOutput(state, bytes("efgh"), 16);

    expect(text(takePtyOutputBatch(state, 5))).toBe("abcde");
    expect(state.totalBytes).toBe(3);
    expect(text(takePtyOutputBatch(state, 16))).toBe("fgh");
    expect(state.totalBytes).toBe(0);
  });
});
