export function sniFromHello(data) {
  if (data.length < 5) return null;
  if (data[0] !== 22 || data[1] !== 3) throw new Error("CONNECT did not start with a TLS ClientHello");
  const recordEnd = 5 + data.readUInt16BE(3);
  if (recordEnd > 65_536) throw new Error("TLS ClientHello too large");
  if (data.length < recordEnd) return null;
  let cursor = 5;
  if (data[cursor++] !== 1) throw new Error("Expected a TLS ClientHello");
  const helloLength = data.readUIntBE(cursor, 3); cursor += 3;
  if (cursor + helloLength !== recordEnd) throw new Error("Fragmented or malformed TLS ClientHello is not accepted");
  const end = cursor + helloLength;
  const take = (count) => { if (count < 0 || cursor + count > end) throw new Error("Malformed TLS ClientHello"); const at = cursor; cursor += count; return at; };
  take(2 + 32);
  const sessionLength = data[take(1)]; take(sessionLength);
  const cipherLength = data.readUInt16BE(take(2)); if (cipherLength % 2 !== 0) throw new Error("Malformed TLS cipher suites"); take(cipherLength);
  const compressionLength = data[take(1)]; take(compressionLength);
  const extensionsLength = data.readUInt16BE(take(2));
  const extensionsEnd = cursor + extensionsLength;
  if (extensionsEnd !== end) throw new Error("Malformed TLS extensions");
  let serverName = null;
  while (cursor < extensionsEnd) {
    const kind = data.readUInt16BE(take(2));
    const length = data.readUInt16BE(take(2));
    const extensionEnd = cursor + length;
    if (extensionEnd > extensionsEnd) throw new Error("Malformed TLS extension length");
    if (kind === 0) {
      if (serverName !== null) throw new Error("Duplicate TLS SNI extension");
      const namesLength = data.readUInt16BE(take(2));
      const namesEnd = cursor + namesLength;
      if (namesEnd !== extensionEnd) throw new Error("Malformed TLS SNI list");
      if (data[take(1)] !== 0) throw new Error("TLS SNI is not a hostname");
      const nameLength = data.readUInt16BE(take(2));
      const name = data.subarray(take(nameLength), cursor);
      if (cursor !== namesEnd) throw new Error("Expected exactly one TLS SNI hostname");
      if (!name.length || name.some((byte) => byte > 0x7f)) throw new Error("TLS SNI hostname must contain ASCII bytes only");
      serverName = name.toString("latin1");
    }
    cursor = extensionEnd;
  }
  if (serverName === null) throw new Error("TLS SNI missing");
  return serverName;
}
