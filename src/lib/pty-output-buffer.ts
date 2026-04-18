export interface PtyOutputBufferState {
  chunks: Uint8Array[];
  totalBytes: number;
}

export function createPtyOutputBufferState(): PtyOutputBufferState {
  return {
    chunks: [],
    totalBytes: 0,
  };
}

export function concatUint8Arrays(chunks: Uint8Array[]): Uint8Array {
  if (chunks.length === 0) {
    return new Uint8Array(0);
  }

  if (chunks.length === 1) {
    return chunks[0]!;
  }

  let totalLength = 0;
  for (const chunk of chunks) {
    totalLength += chunk.length;
  }

  const merged = new Uint8Array(totalLength);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.length;
  }
  return merged;
}

export function enqueueBoundedPtyOutput(
  state: PtyOutputBufferState,
  chunk: Uint8Array,
  maxBytes: number,
): number {
  if (chunk.length === 0) {
    return 0;
  }

  if (maxBytes <= 0) {
    const dropped = chunk.length + state.totalBytes;
    state.chunks = [];
    state.totalBytes = 0;
    return dropped;
  }

  if (chunk.length >= maxBytes) {
    const dropped = state.totalBytes + (chunk.length - maxBytes);
    state.chunks = [chunk.subarray(chunk.length - maxBytes)];
    state.totalBytes = maxBytes;
    return dropped;
  }

  state.chunks.push(chunk);
  state.totalBytes += chunk.length;

  let dropped = 0;
  while (state.totalBytes > maxBytes && state.chunks.length > 0) {
    const overflow = state.totalBytes - maxBytes;
    const first = state.chunks[0]!;

    if (first.length <= overflow) {
      state.chunks.shift();
      state.totalBytes -= first.length;
      dropped += first.length;
      continue;
    }

    state.chunks[0] = first.subarray(overflow);
    state.totalBytes -= overflow;
    dropped += overflow;
  }

  return dropped;
}

export function takePtyOutputBatch(
  state: PtyOutputBufferState,
  maxBytes: number,
): Uint8Array | null {
  if (state.chunks.length === 0 || state.totalBytes === 0 || maxBytes <= 0) {
    return null;
  }

  if (state.totalBytes <= maxBytes) {
    const batch = concatUint8Arrays(state.chunks);
    state.chunks = [];
    state.totalBytes = 0;
    return batch;
  }

  const selected: Uint8Array[] = [];
  let remaining = maxBytes;

  while (remaining > 0 && state.chunks.length > 0) {
    const first = state.chunks[0]!;

    if (first.length <= remaining) {
      selected.push(first);
      state.chunks.shift();
      state.totalBytes -= first.length;
      remaining -= first.length;
      continue;
    }

    selected.push(first.subarray(0, remaining));
    state.chunks[0] = first.subarray(remaining);
    state.totalBytes -= remaining;
    remaining = 0;
  }

  return concatUint8Arrays(selected);
}
