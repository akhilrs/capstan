#!/usr/bin/env bash
# Derives the ssh-keygen outputs the Keys tests compare against, from the committed test-only private keys.
#   gen-keys.sh          write <name>.public.txt, <name>.fingerprint.txt and ed25519-seed.test-key
#   gen-keys.sh --check  derive them again into a temporary folder and diff against the committed files
#   gen-keys.sh --new    make new private keys first (only when the fixtures must change)
# Needs ssh-keygen and python3. The outputs are not named *.pub, so ssh-keygen never reads a stale comment.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
keys=(p256 ed25519)

derive() { # out-dir
  local out=$1 name
  for name in "${keys[@]}"; do
    ssh-keygen -y -f "$here/$name.test-key" > "$out/$name.public.txt"
    ssh-keygen -lf "$out/$name.public.txt" | awk '{print $2}' > "$out/$name.fingerprint.txt"
  done
  # The 32-byte ed25519 seed, hex, read from the unencrypted OpenSSH private key (PROTOCOL.key).
  python3 - "$here/ed25519.test-key" > "$out/ed25519-seed.test-key" <<'PY'
import base64, struct, sys
text = open(sys.argv[1]).read()
body = "".join(l for l in text.splitlines() if l and not l.startswith("-----"))
data = base64.b64decode(body)
magic = b"openssh-key-v1\0"
assert data.startswith(magic)
pos = len(magic)
def string():
    global pos
    (n,) = struct.unpack(">I", data[pos:pos + 4])
    pos += 4
    value = data[pos:pos + n]
    pos += n
    return value
assert string() == b"none" and string() == b"none" and string() == b""
(count,) = struct.unpack(">I", data[pos:pos + 4]); pos += 4
assert count == 1
string()                                  # public key blob
private = string()
pos, data = 8, private                    # skip the two check integers
assert string() == b"ssh-ed25519"
public = string()
secret = string()                         # seed (32) followed by the public key (32)
assert len(secret) == 64 and secret[32:] == public
print(secret[:32].hex())
PY
}

case "${1:-}" in
  --new)
    rm -f "$here"/p256.test-key "$here"/ed25519.test-key
    ssh-keygen -q -t ecdsa -b 256 -m PEM -N "" -C "capstan-test" -f "$here/p256.test-key"
    ssh-keygen -q -t ed25519 -N "" -C "capstan-test" -f "$here/ed25519.test-key"
    rm -f "$here"/p256.test-key.pub "$here"/ed25519.test-key.pub
    derive "$here"
    ;;
  --check)
    tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
    derive "$tmp"
    status=0
    for file in p256.public.txt p256.fingerprint.txt ed25519.public.txt ed25519.fingerprint.txt ed25519-seed.test-key; do
      diff -u "$here/$file" "$tmp/$file" || status=1
    done
    [ $status -eq 0 ] && echo "gen-keys: committed outputs match ssh-keygen"
    exit $status
    ;;
  "") derive "$here" ;;
  *) echo "usage: gen-keys.sh [--check | --new]" >&2; exit 2 ;;
esac
