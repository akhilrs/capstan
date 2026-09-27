const decoder = new TextDecoder("utf-8", { fatal: true });

export function createReceiptFrameBuffer(maxBytes) {
  if (!Number.isSafeInteger(maxBytes) || maxBytes < 1) throw new RangeError("maxBytes must be a positive safe integer");
  let chunks = [];
  let byteLength = 0;
  let complete = false;
  return {
    push(chunk) {
      if (!(chunk instanceof Uint8Array)) throw new TypeError("receipt chunks must be bytes");
      if (complete) {
        if (chunk.length) throw new Error("multiple receipt frames on one connection");
        return null;
      }
      const newline = chunk.indexOf(0x0a);
      const length = newline < 0 ? chunk.length : newline;
      if (byteLength + length + (newline < 0 ? 0 : 1) > maxBytes) throw new RangeError("receipt frame exceeds byte limit");
      chunks.push(chunk.subarray(0, length));
      byteLength += length;
      if (newline < 0) return null;
      if (newline !== chunk.length - 1) throw new Error("multiple receipt frames on one connection");
      complete = true;
      const bytes = Buffer.concat(chunks, byteLength);
      if (byteLength >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf)
        throw new Error("receipt frame has an initial UTF-8 BOM");
      chunks = [];
      return decoder.decode(bytes);
    },
  };
}
