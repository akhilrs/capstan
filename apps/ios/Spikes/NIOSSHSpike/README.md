# NIOSSHSpike

What swift-nio-ssh 0.15.1 can do for Capstan for iPhone (package `ssh-spike` of `docs/plans/iphone-app-plan.md`, section 5.3). Nothing depends on this package; it stays as the record.

```sh
swift test --package-path apps/ios/Spikes/NIOSSHSpike 2>&1 | tail -60
```

Every test prints `SPIKE item <n>: ...` lines. Servers and clients use only `127.0.0.1`.

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
