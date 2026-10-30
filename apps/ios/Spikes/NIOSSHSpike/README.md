# NIOSSHSpike

What swift-nio-ssh 0.15.1 can do for Capstan for iPhone (package `ssh-spike` of `docs/plans/iphone-app-plan.md`, section 5.3). Nothing depends on this package; it stays as the record.

```sh
swift test --package-path apps/ios/Spikes/NIOSSHSpike 2>&1 | tail -60
```

Every test prints `SPIKE item <n>: ...` lines. Servers listen on `0.0.0.0` (every interface, a decision made for this spike); the client connects to `127.0.0.1`. With the macOS firewall on, the first run may ask whether to accept incoming connections: the tests pass either way, as loopback is always allowed.

| Item | Test | What it shows |
|---|---|---|
| 1 | `testItem1SecureEnclaveUserAuth` | userauth signed by a Secure Enclave P-256 key (skipped without an enclave) |
| 2 | `testItem2OutboundSignal` | the client sends a signal request |
| 3 | `testItem3ExitSignalReceived` | the client receives exit-signal |
| 4 | `testItem4StdinEOFAfterExec` | stdin EOF after exec; stdout, stderr and exit-status come back |
| 5 | `testItem5KeyExchanges` | curve25519-sha256, ecdh-sha2-nistp256/384/521, each offered alone by a probe server |
| 6 | `testItem6HostKeys` | ssh-ed25519, ecdsa-sha2-nistp256/384/521 host keys, full handshake |
| 7 | `testItem7Ciphers` | aes128-gcm@openssh.com, aes256-gcm@openssh.com, full session |
| 8 | `testItem8HostKeyOrderProbe` | whether the client's host-key order can be set (no setting: "fixed") |

The library's key-exchange list is fixed on both sides, so item 5 uses `KexProbeServer`: it sends a KEXINIT with one algorithm and checks that the client answers with KEX_ECDH_INIT carrying a key of the right size. The exchange is not completed for algorithms other than the client's first choice.

## Results (Gate G3)

Run on the user's Mac, Xcode 26.6, swift-nio-ssh 0.15.1: `Executed 8 tests, with 0 failures`.

| Item | Test | Expected | Result |
|---|---|---|---|
| 1 | `testItem1SecureEnclaveUserAuth` | passed, or skipped without an enclave | passed: an ecdsa-sha2-nistp256 signature from the Secure Enclave was accepted |
| 2 | `testItem2OutboundSignal` | passed | passed: the server received signal TERM |
| 3 | `testItem3ExitSignalReceived` | passed | passed: the client received exit-signal KILL |
| 4 | `testItem4StdinEOFAfterExec` | passed | passed: EOF reached the server; stdout, stderr and exit-status 3 came back |
| 5 | `testItem5KeyExchanges` | 4 × passed | passed: curve25519-sha256, ecdh-sha2-nistp256, ecdh-sha2-nistp384, ecdh-sha2-nistp521 |
| 6 | `testItem6HostKeys` | 4 × passed | passed: ssh-ed25519, ecdsa-sha2-nistp256, ecdsa-sha2-nistp384, ecdsa-sha2-nistp521 |
| 7 | `testItem7Ciphers` | 2 × passed | passed: aes128-gcm@openssh.com, aes256-gcm@openssh.com |
| 8 | `testItem8HostKeyOrderProbe` | recorded | fixed: ssh-ed25519, ecdsa-sha2-nistp384, ecdsa-sha2-nistp256, ecdsa-sha2-nistp521 |

What follows for the plan (section 5.3 outcomes):

- Enclave signing works: the default key stays P-256 in the Secure Enclave (D3); no re-plan (R2 closed for `swift test`; device check 1 still confirms on a phone).
- Signals and exit-signal work: `ssh-tuning` can send a signal before closing, and exit codes map as planned (`128 + n`).
- Extended data (stderr) works: `stderrTail` can be filled.
- No algorithm is missing: the `ssh-tuning` list keeps all four key exchanges, all four host keys and both ciphers.
- The host-key order is fixed: a host whose pinned key is of another kind than the client's first match is reported as changed (the safe side).
- The client's key-exchange order is fixed too: ecdh-sha2-nistp384, ecdh-sha2-nistp256, ecdh-sha2-nistp521, curve25519-sha256, curve25519-sha256@libssh.org. Against OpenSSH the client picks ecdh-sha2-nistp384, not curve25519.
