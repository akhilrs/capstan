#!/usr/bin/env bash
# Static guards for the iPhone app and its packages. A guard, not a proof: it cannot show that the code compiles.
#   check-static.sh              check the tree this script lives in
#   check-static.sh --self-test  plant each pattern in a temporary copy and expect a finding
#
# Rule 8's deny-list: the CapstanKit types that carry text from a host or an agent (kept in step with UntrustedText).
# Code that feeds a Live Activity, a widget or a notification may not name them.
TEXT_TYPES='UntrustedText|WaitingItem|StoredWaiting|StoredAgent|StoredPlan|AgentItem|PlanItem|PromptOption|PromptBody|OperatorBody|DesignBody|MessageBody|EventBody|RunBody|AgentBody|PackageDetail|InboxRow|StreamLine|DecideRefusal|DecideAnswer|SessionError|StopReason|NotSentReason|HostHello|IntegrationArea|IntegrationMigration|IntegrationNextStep|IntegrationPlan|IntegrationReport|IntegrationSignoff|ItemDetail'
set -u

# Files (relative to the repository root) that may open a link.
ALLOW_OPEN=(
  "apps/ios/CapstanPhone/Detail/DesignLinkConfirm.swift"
)

# Rule 6 program (perl): prints "path:line:text" for each finding. Arguments: files.
IFS= read -r -d '' RULE6_PERL <<'PERL'
my $names = qr/(?<![A-Za-z0-9_.])(?:Text|Label|Button|Toggle|Picker|TextField|Section|Menu|LocalizedStringKey)\(|\.(?:help|navigationTitle|accessibilityLabel)\(|(?<![A-Za-z0-9_.])AttributedString\(markdown:/;
while (<>) {
  my $line = $_; chomp $line;
  if ($line !~ m{^\s*//} && $line !~ m{//\s*static-text\s*$}) {
    while ($line =~ /$names/g) {
      my $matched = $&;
      my $rest = substr($line, pos($line));
      $rest =~ s/^\s+//;
      next if $rest eq '' || $rest =~ /^"""/;
      next if $rest =~ /^verbatim\s*:/;
      next if $rest =~ /^[A-Za-z_][A-Za-z0-9_]*\s*:/;
      if ($rest =~ /^"/) {
        if ($rest =~ /^"((?:[^"\\]|\\.)*)"/) { next unless $1 =~ /\\\(/ } else { next }
      }
      print "$ARGV:$.:$line\n";
      last;
    }
  }
  close ARGV if eof;
}
PERL

# Prints "file:line: text" for each finding. Argument: the repository root.
run_checks() {
  local root=$1 ios=$1/apps/ios found=0 out
  [ -d "$ios" ] || { echo "no apps/ios under $root" >&2; return 2; }

  report() { # rule, grep output (path:line:text) -> relative file:line
    local rule=$1 line
    while IFS= read -r line; do
      [ -n "$line" ] || continue
      echo "${line#"$root"/}" | sed -E "s/^([^:]+:[0-9]+):/\1: [$rule] /"
      found=1
    done
  }

  # (1) CapstanPhoneCore imports only what plan section 2.2 allows; CapstanSSH may add NIO and CapstanPhoneCore
  local dir allowed_imports
  for dir in "CapstanPhoneCore|Foundation|CryptoKit|Security|LocalAuthentication|CapstanKit|CapstanAppCore" \
    "CapstanSSH|Foundation|CryptoKit|Security|CapstanKit|CapstanAppCore|CapstanPhoneCore|NIOCore|NIOPosix|NIOSSH|NIOConcurrencyHelpers"; do
    allowed_imports=${dir#*|}; dir=${dir%%|*}
    [ -d "$ios/$dir/Sources" ] || continue
    out=$(grep -rnE --include='*.swift' '^[[:space:]]*(@[A-Za-z_]+[[:space:]]+)*import[[:space:]]' "$ios/$dir/Sources" \
      | grep -vE ":[[:space:]]*(@[A-Za-z_]+[[:space:]]+)*import[[:space:]]+($allowed_imports)[[:space:]]*(//.*)?\$")
    report "import" <<< "$out"
  done

  # (2) no markup, rich-text or link detection in any Swift file
  out=$(grep -rnE --include='*.swift' \
    'AttributedString\(markdown|NSAttributedString\(html|Text\(\.init\(|LocalizedStringKey\(|\.linkDetection|dataDetector|NSDataDetector' "$ios")
  report "markup" <<< "$out"

  # (3) links are opened only from the allowed files
  out=$(grep -rnE --include='*.swift' '(^|[^A-Za-z0-9_])Link\(|(^|[^A-Za-z0-9_])openURL|UIApplication\.shared\.open\(' "$ios")
  if [ -n "$out" ]; then
    local allowed line file
    while IFS= read -r line; do
      file=${line%%:*}; file=${file#"$root"/}
      allowed=0
      for a in "${ALLOW_OPEN[@]}"; do [ "$file" = "$a" ] && allowed=1; done
      [ $allowed = 1 ] || report "open" <<< "$line"
    done <<< "$out"
  fi

  # (4) no try! or fatalError in the package sources
  for dir in CapstanPhoneCore CapstanSSH; do
    [ -d "$ios/$dir/Sources" ] || continue
    out=$(grep -rnE --include='*.swift' 'try!|fatalError\(' "$ios/$dir/Sources")
    report "trap" <<< "$out"
  done

  # (5) no user-home path literal outside tests
  out=$(grep -rnE --include='*.swift' '/Users/|/home/' "$ios" | grep -vE '/[A-Za-z]*Tests/')
  report "path" <<< "$out"

  # (6) user-visible text in the app is a plain literal or verbatim
  if [ -d "$ios/CapstanPhone" ]; then
    out=$(find "$ios/CapstanPhone" -name '*.swift' -print0 | xargs -0 perl -e "$RULE6_PERL" 2>/dev/null)
    report "text" <<< "$out"
  fi

  # (7) no push: no aps-environment, no remote notification registration, no control titled with "push" (a text
  # that says "not pushed" is allowed)
  out=$(grep -rnE --include='*.swift' --include='*.plist' --include='*.entitlements' --include='*.pbxproj' \
    'aps-environment|registerForRemoteNotifications' "$ios" | grep -vE '/scripts/')
  report "push" <<< "$out"
  if [ -d "$ios/CapstanPhone" ]; then
    out=$(grep -rnE --include='*.swift' '(Button|Toggle|Label|Menu|Text)\("[^"]*[Pp]ush' "$ios/CapstanPhone" \
      | grep -viE 'not pushed')
    report "push" <<< "$out"
  fi

  # (8) Live Activity, widget and notification code never names a type that carries host or agent text
  local surface
  surface=$(grep -rlE --include='*.swift' '^[[:space:]]*(@[A-Za-z_]+[[:space:]]+)*import[[:space:]]+(ActivityKit|WidgetKit|UserNotifications)' "$ios" \
    | grep -vE '/[A-Za-z]*Tests/')
  if [ -n "$surface" ]; then
    while IFS= read -r file; do
      out=$(grep -nE "(^|[^A-Za-z0-9_])($TEXT_TYPES)([^A-Za-z0-9_]|\$)" "$file" | sed "s|^|$file:|")
      report "surface" <<< "$out"
    done <<< "$surface"
  fi

  # (9) the shared-platform check and the project file check, when they exist
  if [ -f "$ios/scripts/check-shared-platform.py" ]; then
    if ! out=$(python3 -I "$ios/scripts/check-shared-platform.py" "$root" 2>&1); then
      echo "apps/ios/scripts/check-shared-platform.py:1: [platform] $(echo "$out" | head -n 1)"; found=1
    fi
  fi
  if [ -f "$ios/scripts/check-xcodeproj.py" ]; then
    if ! out=$(python3 -I "$ios/scripts/check-xcodeproj.py" "$ios/CapstanPhone.xcodeproj" 2>&1); then
      echo "apps/ios/scripts/check-xcodeproj.py:1: [xcodeproj] $(echo "$out" | head -n 1)"; found=1
    fi
  fi

  return $found
}

self_test() {
  local here root tmp rc=0 name pattern content
  here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
  root=$(cd "$here/../../.." && pwd)
  tmp=$(mktemp -d "${TMPDIR:-/tmp}/check-static.XXXXXX") || return 1
  trap 'rm -rf "$tmp"' RETURN

  fresh() {
    rm -rf "$tmp/repo"; mkdir -p "$tmp/repo/apps"
    cp -R "$root/apps/ios" "$tmp/repo/apps/ios"
    cp -R "$root/apps/macos" "$tmp/repo/apps/macos"
    find "$tmp/repo/apps" -name .build -type d -prune -exec rm -rf {} +
  }

  fresh
  if ! run_checks "$tmp/repo" >"$tmp/clean.out" 2>&1; then
    echo "self-test: the unmodified tree has findings" >&2; cat "$tmp/clean.out" >&2; return 1
  fi

  # name | file under apps/ios | content ("~" starts a new line)
  local cases=(
    "import-core|CapstanPhoneCore/Sources/CapstanPhoneCore/Planted.swift|import SwiftUI"
    "import-core-nio|CapstanPhoneCore/Sources/CapstanPhoneCore/Planted.swift|import NIOCore"
    "import-attr|CapstanPhoneCore/Sources/CapstanPhoneCore/Planted.swift|@preconcurrency import UIKit"
    "import-ssh|CapstanSSH/Sources/CapstanSSH/Planted.swift|import SwiftUI"
    "markup-attributed|CapstanPhone/Planted.swift|let a = AttributedString(markdown: s)"
    "markup-text|CapstanPhone/Planted.swift|let a = Text(.init(s))"
    "markup-key|CapstanPhone/Planted.swift|let a = LocalizedStringKey(s)"
    "markup-detector|CapstanPhone/Planted.swift|let t = NSDataDetector(types: 0)"
    "open-link|CapstanPhone/Planted.swift|let a = Link(\"x\", destination: u)"
    "open-app|CapstanPhone/Planted.swift|UIApplication.shared.open(u)"
    "open-env|CapstanPhone/Planted.swift|@Environment(\\.openURL) var o"
    "trap-try|CapstanPhoneCore/Sources/CapstanPhoneCore/Planted.swift|let a = try! f()"
    "trap-fatal|CapstanSSH/Sources/CapstanSSH/Planted.swift|fatalError(\"x\")"
    "path-users|CapstanPhone/Planted.swift|let p = \"/Users/someone\""
    "text-Text|CapstanPhone/Planted.swift|let a = Text(name)"
    "text-Button|CapstanPhone/Planted.swift|let a = Button(name) { }"
    "text-title|CapstanPhone/Planted.swift|view.navigationTitle(name)"
    "text-interpolated|CapstanPhone/Planted.swift|let a = Text(\"hi \\(name)\")"
    "push-aps|CapstanPhone/Planted.swift|let k = \"aps-environment\""
    "push-register|CapstanPhone/Planted.swift|UIApplication.shared.registerForRemoteNotifications()"
    "push-title|CapstanPhone/Planted.swift|let a = Toggle(\"Push after merge\", isOn: \$b)"
    "surface-widget|CapstanWidgets/Planted.swift|import WidgetKit~let s: UntrustedText? = nil"
    "surface-notification|CapstanPhone/Planted.swift|import UserNotifications~let w: WaitingItem? = nil"
  )
  local c file
  for c in "${cases[@]}"; do
    IFS='|' read -r name file content <<< "$c"
    fresh
    mkdir -p "$(dirname "$tmp/repo/apps/ios/$file")"
    printf '%s\n' "${content//\~/$'\n'}" > "$tmp/repo/apps/ios/$file"
    if run_checks "$tmp/repo" >"$tmp/case.out" 2>&1; then
      echo "self-test: '$name' was not caught" >&2; rc=1
    elif ! grep -q 'Planted.swift:[12]:' "$tmp/case.out"; then
      echo "self-test: '$name' failed without a file:line finding" >&2; cat "$tmp/case.out" >&2; rc=1
    fi
  done

  # An allowed file may open links; receiving a link (onOpenURL) opens nothing; a test file may hold a path; "not
  # pushed" is allowed text; a surface file with only counts and kinds is fine.
  fresh
  mkdir -p "$tmp/repo/apps/ios/CapstanPhone/Detail" "$tmp/repo/apps/ios/CapstanPhoneCore/Tests/CapstanPhoneCoreTests" \
    "$tmp/repo/apps/ios/CapstanWidgets"
  echo 'UIApplication.shared.open(u)' > "$tmp/repo/apps/ios/CapstanPhone/Detail/DesignLinkConfirm.swift"
  echo 'let p = "/home/x"' > "$tmp/repo/apps/ios/CapstanPhoneCore/Tests/CapstanPhoneCoreTests/Planted.swift"
  printf 'let a = Text("Merged, not pushed")\nview.onOpenURL { url in model.handle(url: url) }\n' > "$tmp/repo/apps/ios/CapstanPhone/Planted.swift"
  printf 'import WidgetKit\nlet count: Int = 0\nlet kind: WaitingKind = .op\n' > "$tmp/repo/apps/ios/CapstanWidgets/Counts.swift"
  if ! run_checks "$tmp/repo" >"$tmp/allow.out" 2>&1; then
    echo "self-test: an allowed pattern was flagged" >&2; cat "$tmp/allow.out" >&2; rc=1
  fi

  # Plain literals, verbatim, labelled arguments and look-alike names are not findings.
  fresh
  cat > "$tmp/repo/apps/ios/CapstanPhone/Planted.swift" <<'SWIFT'
let a = MyText(name)
let c = Text("plain")
let d = Text(verbatim: name)
let e = Button(action: go) { }
let g = Text(name) // static-text
SWIFT
  if ! run_checks "$tmp/repo" >"$tmp/ok6.out" 2>&1; then
    echo "self-test: a rule 6 non-finding was flagged" >&2; cat "$tmp/ok6.out" >&2; rc=1
  fi

  # A failing check-xcodeproj.py or check-shared-platform.py fails the script.
  for helper in check-xcodeproj.py check-shared-platform.py; do
    fresh
    printf 'import sys\nsys.exit(1)\n' > "$tmp/repo/apps/ios/scripts/$helper"
    if run_checks "$tmp/repo" >"$tmp/xc.out" 2>&1; then
      echo "self-test: a failing $helper was not caught" >&2; rc=1
    fi
  done

  [ $rc = 0 ] && echo "self-test passed (${#cases[@]} planted patterns caught)"
  return $rc
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
  exit $?
fi

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
if run_checks "$(cd "$here/../../.." && pwd)"; then
  echo "check-static: ok"
else
  echo "check-static: findings above" >&2
  exit 1
fi
