# Test-only keys

**These keys are for tests only. They are public, they protect nothing, and they must never be added to any `authorized_keys` or used for a real host.**

| File | What it is |
|---|---|
| `p256.test-key` | ECDSA P-256 private key, PEM (`EC PRIVATE KEY`); the public key type of the Secure Enclave key |
| `ed25519.test-key` | ed25519 private key, OpenSSH format |
| `ed25519-seed.test-key` | the 32-byte seed of `ed25519.test-key`, hex (derived) |
| `<name>.public.txt` | `ssh-keygen -y -f <name>.test-key` (derived) |
| `<name>.fingerprint.txt` | the SHA256 field of `ssh-keygen -lf <name>.public.txt` (derived) |

`gen-keys.sh` writes the derived files; `gen-keys.sh --check` derives them again and diffs; `gen-keys.sh --new` makes new private keys. The Swift tests read only the committed files.
