# Device checks

What no unit test settles, run by hand on a physical iPhone (plan section 6.3). The screen is in the app:
**Hosts → Settings → long-press Version**. It shows passed or failed with the text of each check, and **Copy results**
puts the table below on the clipboard, ready to paste.

Before you start: a build on the iPhone from Xcode, at least one host added and live (Waiting says "live"), and the
host on the same Wi-Fi as the phone for check 3.

| # | Check | How | Expected |
|---|---|---|---|
| 1 | Secure Enclave key signs | Tap **Run**. A throwaway enclave key signs a payload and the signature is verified. | Passed: "Signed and verified". A failure here starts the ed25519-default re-plan (plan section 10, R2). |
| 2 | Face ID unlocks | Tap **Run** and look at the phone. | Passed: "Unlocked". Cancel gives Failed. |
| 3 | Local Network prompt | Delete the app first so iOS asks again. Install, add a LAN host, and watch for the "find devices on your local network" prompt on the first connection. Then deny it in iOS Settings → Privacy → Local Network, and run **the connection check**. | The prompt appears once. With access denied, the check fails with a readable line, not a timeout with no words. Allow it again afterwards. |
| 4 | SSH login with host-key trust | Choose the host, tap **Run the connection check**. For a host the phone has not trusted, the trust sheet shows the fingerprint first. | Every step passes: connected, Capstan answers, projects found, first snapshot. |
| 5 | cstan events for 10 s | Choose a live host, tap **Watch for 10 s**. | Several distinct line times; on a busy project, cursors advance. |
| 6 | Dead link | With the host live, tap **Start**, then turn off the phone's Wi-Fi (or the host's network). Wait. | "No heartbeat" after about 3 poll intervals, and "Channel ended" after about 60 s (SSH keepalive). Turn Wi-Fi back on. |
| 7 | Face ID after time away | With Face ID on in Settings: go to the Home Screen for 30 s, come back, and tap **Asked** or **Not asked**. Repeat for 90 s. | 30 s: not asked. 90 s: asked. |

Paste the copied table into the PR or the chat, with the iPhone model and the iOS version (the table ends with them).
