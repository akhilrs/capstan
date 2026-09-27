import { TextDecoder } from "node:util";

export const M1_MAX_FRAME_BYTES = 1_048_576;
export const M1_MAX_PROMPT_BYTES = 262_144;

export type M1Frame = Record<string, unknown>;

const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

export function parseM1Frame(buffer: Buffer): M1Frame {
  if (
    buffer.length === 0 ||
    buffer.length > M1_MAX_FRAME_BYTES ||
    buffer[buffer.length - 1] !== 0x0a
  )
    throw new Error("expected one newline-terminated frame");
  const newline = buffer.indexOf(0x0a);
  if (newline !== buffer.length - 1 || newline === 0)
    throw new Error("multiple or empty frames are forbidden");
  let value: unknown;
  try {
    value = JSON.parse(decoder.decode(buffer.subarray(0, newline)));
  } catch {
    throw new Error("frame is not valid UTF-8 JSON");
  }
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("frame must be a JSON object");
  return value as M1Frame;
}

export class M1ResponseFrameParser {
  #pending: Buffer<ArrayBufferLike> = Buffer.alloc(0);
  #response: M1Frame | undefined;

  push(chunk: Buffer): M1Frame[] {
    const data = this.#pending.length
      ? Buffer.concat([this.#pending, chunk])
      : chunk;
    const frames: M1Frame[] = [];
    let offset = 0;
    while (offset < data.length) {
      const newline = data.indexOf(0x0a, offset);
      if (newline < 0) break;
      const frame = parseM1Frame(data.subarray(offset, newline + 1));
      if (this.#response)
        throw new Error("unexpected frame after bridge response");
      this.#response = frame;
      frames.push(frame);
      offset = newline + 1;
    }
    this.#pending = data.subarray(offset);
    if (this.#pending.length > M1_MAX_FRAME_BYTES)
      throw new Error("bridge frame exceeds the frame limit");
    if (this.#pending.length) return [];
    return frames;
  }

  finish(): void {
    if (this.#pending.length)
      throw new Error("bridge closed with an incomplete frame");
  }
}
