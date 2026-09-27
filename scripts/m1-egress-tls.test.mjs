import assert from "node:assert/strict";
import { sniFromHello } from "./m1-egress-tls.mjs";

const u16 = (value) => { const bytes = Buffer.alloc(2); bytes.writeUInt16BE(value); return bytes; };
const extension = (kind, payload) => Buffer.concat([u16(kind), u16(payload.length), payload]);
const sniExtension = (hostname) => {
  const name = Buffer.isBuffer(hostname) ? hostname : Buffer.from(hostname, "ascii");
  const entry = Buffer.concat([Buffer.from([0]), u16(name.length), name]);
  return extension(0, Buffer.concat([u16(entry.length), entry]));
};
const hello = (...extensions) => {
  const ext = Buffer.concat(extensions);
  const body = Buffer.concat([
    Buffer.from([3, 3]), Buffer.alloc(32), Buffer.from([0]),
    u16(2), Buffer.from([0x13, 0x01]), Buffer.from([1, 0]), u16(ext.length), ext,
  ]);
  const handshake = Buffer.concat([Buffer.from([1, 0, 0, body.length]), body]);
  return Buffer.concat([Buffer.from([22, 3, 1]), u16(handshake.length), handshake]);
};

assert.equal(sniFromHello(hello(sniExtension("api.example.com"))), "api.example.com");
assert.throws(() => sniFromHello(hello(sniExtension(Buffer.from([0xc0, 0xaf])))), /ASCII/);
assert.throws(() => sniFromHello(hello(sniExtension("api.example.com"), sniExtension("api.example.com"))), /Duplicate TLS SNI/);
console.log("PASS TLS SNI rejects non-ASCII host bytes and duplicate SNI extensions");
