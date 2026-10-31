# Releasing Capstan for iPhone (TestFlight)

Everything needed to archive and upload a TestFlight build (plan 7.6). Nothing here is run by a session: the person
archives and uploads.

## What it needs first

- **A paid Apple Developer Program membership.** A free Personal Team runs the app on your own iPhone, but cannot
  upload to TestFlight or sign an App Group (which the Home Screen widgets need).
- **The device pass is done** (`docs/device-pass.md` filled, failures fixed).

## Identifiers and capabilities

| Item | Value |
|---|---|
| App bundle id | `in.qusol.capstan.phone` |
| Widget extension bundle id | `in.qusol.capstan.phone.widgets` |
| App Group (for the widgets, once a paid team signs it) | `group.in.qusol.capstan` |
| Background refresh task | `in.qusol.capstan.phone.refresh` (Info.plist `BGTaskSchedulerPermittedIdentifiers`) |
| URL scheme | `capstan` |
| Version / build | `MARKETING_VERSION` 0.1.0 / `CURRENT_PROJECT_VERSION` 1, raise the build for every upload |

In Xcode → the project → each target → **Signing & Capabilities**, with **Automatically manage signing** and your
paid Team:

- **CapstanPhone:** Background Modes → Background fetch (already on); **Time Sensitive Notifications** if you want
  Time Sensitive per kind to break through Focus (without it iOS delivers them at the normal level); **App Groups**
  → `group.in.qusol.capstan` once the `widgets` package adds the widgets.
- **CapstanWidgets:** App Groups → the same group, with `widgets`.
- Keychain needs no capability (the app uses its own access group). Live Activities need no capability: the app's
  Info.plist has `NSSupportsLiveActivities`.
- **Never** add Push Notifications: Capstan has no server and never registers for remote notifications (D8). The
  static checks fail on `aps-environment`.

## Export compliance (R6, your decision)

Capstan carries its own SSH (swift-nio-ssh with CryptoKit: curve25519, P-256, AES-GCM, ChaCha20-Poly1305) to talk to
your own hosts. App Store Connect asks about encryption on every upload unless Info.plist answers it with
`ITSAppUsesNonExemptEncryption`. Standard algorithms used only to secure the app's own connections usually qualify for
the exemption, which would be `ITSAppUsesNonExemptEncryption = NO`; whether that holds for you (and any annual
self-classification report) is your call. Add the key to `CapstanPhone/Info.plist` once decided; until then App
Store Connect asks at upload.

## Privacy

`CapstanPhone/PrivacyInfo.xcprivacy`: no tracking, no collected data, and the required-reason APIs the binary links:
UserDefaults (CA92.1, through the shared CapstanAppCore), file timestamps (C617.1, SwiftNIO's file APIs inside the
container) and system boot time (35F9.1, SwiftNIO's monotonic clock). App Store Connect's privacy questionnaire:
**Data Not Collected**. The connections go only to the hosts the person adds.

## Before every upload

- [ ] `apps/ios/scripts/check-static.sh` and `python3 apps/ios/scripts/check-xcodeproj.py` are green.
- [ ] `swift test --package-path apps/ios/CapstanPhoneCore` and `swift test --package-path apps/ios/CapstanSSH` are green.
- [ ] No preview data is reachable from app code: `grep -rn "AppModel(preview" apps/ios/CapstanPhone | grep -v "#Preview\|PreviewProvider"` prints only lines inside `#Preview` blocks.
- [ ] The build number is higher than the last upload.
- [ ] The icon shows in the Simulator's Home Screen (Assets → AppIcon, 1024 px, no transparency).
- [ ] The app opens with no host (onboarding) and with one (Face ID, then Waiting).

## Archive and upload

```sh
xcodebuild -project apps/ios/CapstanPhone.xcodeproj -scheme CapstanPhone -configuration Release -destination 'generic/platform=iOS' -archivePath build/CapstanPhone.xcarchive archive
```

Then either Xcode → Window → Organizer → the archive → **Distribute App** → App Store Connect → Upload, or:

```sh
xcodebuild -exportArchive -archivePath build/CapstanPhone.xcarchive -exportPath build/export -exportOptionsPlist apps/ios/docs/ExportOptions.plist
```

and upload `build/export/CapstanPhone.ipa` with Transporter.

## TestFlight test information

> **What to test.** Capstan follows the agents your Capstan hosts run, over SSH with a key made on this iPhone.
> Add a host (Hosts → +), put this iPhone's public line in the host's `~/.ssh/authorized_keys`, trust the host key,
> and the Waiting tab shows what your agents wait for. Try a permission prompt (forwarded to the PM), deny a command
> proposal, pause an agent, and lock the phone to see the Live Activity. Notifications say the kind and the project
> only. Capstan has no server: nothing leaves the phone except to your own hosts.
>
> **Beta App Review sign-in.** Capstan needs a host the reviewer can reach over SSH. Provide a test host and its
> address, user and port in the review notes, with this build's public line already authorized there.
