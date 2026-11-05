# Capstan for iPhone in the car

**Time Sensitive kinds are off by default. Nothing is announced until you turn kinds on** in Capstan → Settings →
Notifications → Time Sensitive.

**Before the first build:** Time Sensitive Notifications must be on the App ID, and the provisioning profile must be
new. Signing fails on a device or an archive without it; see [release.md](release.md).

## Your one-time steps

1. In iOS **Settings → Notifications → Announce Notifications**, turn it on, then under **CarPlay** switch on
   **Capstan**.
2. In **Capstan → Settings → Notifications → Time Sensitive**, switch on the kinds you want read aloud (a command to
   approve, a permission prompt and so on). Each kind starts off.
3. Allow Time Sensitive for Capstan in iOS **Settings → Notifications → Capstan** if iOS offers the choice.

## What is announced, and when

- Siri reads a Time Sensitive notification aloud in CarPlay when Announce Notifications is on for Capstan. It says the
  kind and the project only ("Command to approve. In payments-api."), never the command or any agent text.
- These are local notifications. Capstan has no server and no push (D8), so one is made only while the app is running
  or a background refresh runs. If iOS does not wake the app, nothing is announced.
- Only the kinds you switched on are Time Sensitive; the others arrive at the normal level.
- Apple may still not announce a third-party Time Sensitive notification in some cars; the car pass records what each
  car does.

## The ntfy path

The ntfy iOS app has its own notifications. Whether and when it is Time Sensitive is in
[ntfy: In the car](../../../docs/reference/ntfy.md#in-the-car).

## Sources

- https://github.com/binwiederhier/ntfy-ios/blob/main/ntfy/Utils/NotificationContent.swift (ntfy priority to interruption level), accessed 2026-11-06
- https://github.com/binwiederhier/ntfy/blob/main/server/server_firebase.go (the iOS message is mutable, so ntfy's notification service sets the level), accessed 2026-11-06
- Announce Notifications and CarPlay steps: the repository's `docs/design/plan-156-design-voice.md` (not checked against Apple's pages), accessed 2026-11-06
