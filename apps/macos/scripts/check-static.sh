#!/usr/bin/env bash
# Static guards for the macOS client sources. A guard, not a proof: it cannot show that the code compiles.
#   check-static.sh              check the tree this script lives in
#   check-static.sh --self-test  plant each pattern in a temporary copy and expect a finding
set -u

# Rule 9's deny-list: the CapstanKit types of messages, whose text comes from an agent or the person. Notification and
# menu bar code may not name them.
MESSAGE_TYPES='StoredMessage|MessageItem'

# Files (relative to the repository root) that may open a link.
ALLOW_OPEN=(
  "apps/macos/Capstan/Inbox/DesignLinkConfirm.swift"
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
  local root=$1 macos=$1/apps/macos found=0 out
  [ -d "$macos" ] || { echo "no apps/macos under $root" >&2; return 2; }

  report() { # rule, grep output (path:line:text) -> relative file:line
    local rule=$1 line
    while IFS= read -r line; do
      [ -n "$line" ] || continue
      echo "${line#"$root"/}" | sed -E "s/^([^:]+:[0-9]+):/\1: [$rule] /"
      found=1
    done
  }

  # (1) the kit imports nothing but Foundation, Dispatch and CryptoKit; the app core may also import the kit
  local dir allowed_imports
  for dir in "CapstanKit|Foundation|Dispatch|CryptoKit" "CapstanAppCore|Foundation|Dispatch|CapstanKit"; do
    allowed_imports=${dir#*|}; dir=${dir%%|*}
    [ -d "$macos/$dir/Sources" ] || continue
    out=$(grep -rnE --include='*.swift' '^[[:space:]]*(@[A-Za-z_]+[[:space:]]+)*import[[:space:]]' "$macos/$dir/Sources" \
      | grep -vE ":[[:space:]]*(@[A-Za-z_]+[[:space:]]+)*import[[:space:]]+($allowed_imports)[[:space:]]*(//.*)?\$")
    report "import" <<< "$out"
  done

  # (2) no markup, rich-text or link detection in any Swift file
  out=$(grep -rnE --include='*.swift' \
    'AttributedString\(markdown|NSAttributedString\(html|Text\(\.init\(|LocalizedStringKey\(|\.linkDetection|dataDetector' "$macos")
  report "markup" <<< "$out"

  # (3) links are opened only from the allowed files
  out=$(grep -rnE --include='*.swift' '(^|[^A-Za-z0-9_])Link\(|openURL|NSWorkspace\.shared\.open' "$macos")
  if [ -n "$out" ]; then
    local allowed line file
    while IFS= read -r line; do
      file=${line%%:*}; file=${file#"$root"/}
      allowed=0
      for a in "${ALLOW_OPEN[@]}"; do [ "$file" = "$a" ] && allowed=1; done
      [ $allowed = 1 ] || report "open" <<< "$line"
    done <<< "$out"
  fi

  # (4) no try! or fatalError in the kit sources
  if [ -d "$macos/CapstanKit/Sources" ]; then
    out=$(grep -rnE --include='*.swift' 'try!|fatalError\(' "$macos/CapstanKit/Sources")
    report "trap" <<< "$out"
  fi

  # (5) no user-home path literal outside tests
  out=$(grep -rnE --include='*.swift' '/Users/|/home/' "$macos" | grep -vE '/[A-Za-z]*Tests/')
  report "path" <<< "$out"

  # (6) user-visible text in the app is a plain literal or verbatim
  if [ -d "$macos/Capstan" ]; then
    out=$(find "$macos/Capstan" -name '*.swift' -print0 | xargs -0 perl -e "$RULE6_PERL" 2>/dev/null)
    report "text" <<< "$out"
  fi

  # (7) a stub file that still carries its marker keeps the lines listed in stubs.txt
  local stubs=$macos/scripts/stubs.txt n=0 spath sline
  if [ -f "$stubs" ]; then
    while IFS=$'\t' read -r spath sline || [ -n "$spath" ]; do
      n=$((n + 1))
      [ -n "$spath" ] || continue
      if [ ! -f "$root/$spath" ]; then
        echo "apps/macos/scripts/stubs.txt:$n: [stub] listed file does not exist: $spath"; found=1
      elif [ "$(head -n 1 "$root/$spath" | tr -d '\r')" = "// capstan-stub" ] \
        && ! grep -qFx -- "$sline" "$root/$spath"; then
        echo "$spath:1: [stub] missing line: $sline"; found=1
      fi
    done < "$stubs"
  fi

  # (9) notification and menu bar code never names a message type
  local surface file
  surface=$(
    { grep -rlE --include='*.swift' '^[[:space:]]*(@[A-Za-z_]+[[:space:]]+)*import[[:space:]]+UserNotifications' "$macos"
      find "$macos/Capstan/System" "$macos/CapstanAppCore/Sources/CapstanAppCore/System" -name '*.swift' 2>/dev/null
      find "$macos/Capstan/App" -name 'MenuBar*.swift' 2>/dev/null
    } | grep -vE '/[A-Za-z]*Tests/' | sort -u)
  if [ -n "$surface" ]; then
    while IFS= read -r file; do
      out=$(grep -nE "(^|[^A-Za-z0-9_])($MESSAGE_TYPES)([^A-Za-z0-9_]|\$)" "$file" | sed "s|^|$file:|")
      report "surface" <<< "$out"
    done <<< "$surface"
  fi

  # (8) the project file check belongs to another package; run it when it exists
  if [ -f "$macos/scripts/check-xcodeproj.py" ]; then
    if ! out=$(python3 -I "$macos/scripts/check-xcodeproj.py" "$macos/Capstan.xcodeproj" 2>&1); then
      echo "apps/macos/scripts/check-xcodeproj.py:1: [xcodeproj] $(echo "$out" | head -n 1)"; found=1
    fi
  fi

  # (11) no unqualified use of a type name that CapstanAppCore and CapstanKit both define (the check lives in apps/ios)
  if [ -f "$root/apps/ios/scripts/check-ambiguous-types.py" ]; then
    if ! out=$(python3 -I "$root/apps/ios/scripts/check-ambiguous-types.py" "$root" 2>&1); then
      echo "$out" | head -n 20; found=1
    fi
  fi

  # (10) the PM fields the readers decode are documented and covered by the kernel fixtures; the overview shows the card
  if [ -f "$macos/scripts/check-pm-fields.py" ]; then
    if ! out=$(python3 -I "$macos/scripts/check-pm-fields.py" "$root" 2>&1); then
      while IFS= read -r line; do
        [ -n "$line" ] || continue
        echo "apps/macos/scripts/check-pm-fields.py:1: [pm-fields] $line"
      done <<< "$out"
      found=1
    fi
  fi
  local overview=$macos/Capstan/Inbox/OverviewView.swift
  if [ -f "$overview" ] && ! grep -q 'PmCardView' "$overview"; then
    echo "apps/macos/Capstan/Inbox/OverviewView.swift:1: [pm-card] OverviewView does not reference PmCardView"; found=1
  fi

  # (11) a stuck PM is announced and withdrawn by the coordinator, and the sidebar row shows it
  local coordinator=$macos/Capstan/System/NotificationCoordinator.swift sidebar=$macos/Capstan/Navigation/SidebarView.swift
  if [ -f "$coordinator" ] && ! grep -q 'pmStuck' "$coordinator"; then
    echo "apps/macos/Capstan/System/NotificationCoordinator.swift:1: [pm-stuck] the coordinator does not handle pmStuck"; found=1
  fi
  if [ -f "$sidebar" ] && ! grep -q 'SidebarText\.pmStuck' "$sidebar"; then
    echo "apps/macos/Capstan/Navigation/SidebarView.swift:1: [pm-stuck] the sidebar does not call SidebarText.pmStuck"; found=1
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
    rm -rf "$tmp/repo"; mkdir -p "$tmp/repo/apps" "$tmp/repo/docs" "$tmp/repo/rust/crates/kernel/tests/rust_only/fixtures"
    cp -R "$root/apps/macos" "$tmp/repo/apps/macos"
    mkdir -p "$tmp/repo/apps/ios/scripts"; cp "$root/apps/ios/scripts/check-ambiguous-types.py" "$tmp/repo/apps/ios/scripts/"
    cp "$root/docs/remote-protocol.md" "$tmp/repo/docs/"
    cp -R "$root/rust/crates/kernel/tests/rust_only/fixtures/event_feed" \
      "$tmp/repo/rust/crates/kernel/tests/rust_only/fixtures/"
    rm -rf "$tmp/repo/apps/macos/CapstanKit/.build" "$tmp/repo/apps/macos/CapstanAppCore/.build"
  }

  fresh
  if ! run_checks "$tmp/repo" >"$tmp/clean.out" 2>&1; then
    echo "self-test: the unmodified tree has findings" >&2; cat "$tmp/clean.out" >&2; return 1
  fi

  # name | file under apps/macos | content
  local cases=(
    "import|CapstanKit/Sources/CapstanKit/Planted.swift|import SwiftUI"
    "import-attr|CapstanKit/Sources/CapstanKit/Planted.swift|@preconcurrency import AppKit"
    "markup-attributed|Capstan/Planted.swift|let a = AttributedString(markdown: s)"
    "markup-html|Capstan/Planted.swift|let a = NSAttributedString(html: d)"
    "markup-text|Capstan/Planted.swift|let a = Text(.init(s))"
    "markup-key|Capstan/Planted.swift|let a = LocalizedStringKey(s)"
    "markup-detect|Capstan/Planted.swift|view.linkDetection = true"
    "markup-detector|Capstan/Planted.swift|let t = dataDetector"
    "open-link|Capstan/Planted.swift|let a = Link(\"x\", destination: u)"
    "open-url|Capstan/Planted.swift|let a = NSWorkspace.shared.open(u)"
    "open-env|Capstan/Planted.swift|@Environment(\\.openURL) var o"
    "trap-try|CapstanKit/Sources/CapstanKit/Planted.swift|let a = try! f()"
    "trap-fatal|CapstanKit/Sources/CapstanKit/Planted.swift|fatalError(\"x\")"
    "path-users|Capstan/Planted.swift|let p = \"/Users/someone\""
    "path-home|CapstanKit/Sources/CapstanKit/Planted.swift|let p = \"/home/someone\""
    "import-core|CapstanAppCore/Sources/CapstanAppCore/Planted.swift|import SwiftUI"
    "text-Text|Capstan/Planted.swift|let a = Text(name)"
    "text-Label|Capstan/Planted.swift|let a = Label(name, systemImage: \"x\")"
    "text-Button|Capstan/Planted.swift|let a = Button(name) { }"
    "text-Toggle|Capstan/Planted.swift|let a = Toggle(name, isOn: \$b)"
    "text-Picker|Capstan/Planted.swift|let a = Picker(name, selection: \$b) { }"
    "text-TextField|Capstan/Planted.swift|let a = TextField(name, text: \$b)"
    "text-Section|Capstan/Planted.swift|let a = Section(name) { }"
    "text-Menu|Capstan/Planted.swift|let a = Menu(name) { }"
    "text-help|Capstan/Planted.swift|view.help(name)"
    "text-title|Capstan/Planted.swift|view.navigationTitle(name)"
    "text-a11y|Capstan/Planted.swift|view.accessibilityLabel(name)"
    "text-key|Capstan/Planted.swift|let a = LocalizedStringKey(name)"
    "text-markdown|Capstan/Planted.swift|let a = AttributedString(markdown: name)"
    "text-interpolated|Capstan/Planted.swift|let a = Text(\"hi \\(name)\")"
    "surface-menu|Capstan/System/Planted.swift|let m: StoredMessage? = nil"
    "surface-core|CapstanAppCore/Sources/CapstanAppCore/System/Planted.swift|let m: MessageItem? = nil"
    "surface-notify|Capstan/Planted.swift|import UserNotifications; let m: StoredMessage? = nil"
  )
  local c file
  for c in "${cases[@]}"; do
    IFS='|' read -r name file content <<< "$c"
    fresh
    mkdir -p "$(dirname "$tmp/repo/apps/macos/$file")"
    printf '%s\n' "$content" > "$tmp/repo/apps/macos/$file"
    if run_checks "$tmp/repo" >"$tmp/case.out" 2>&1; then
      echo "self-test: '$name' was not caught" >&2; rc=1
    elif ! grep -q 'Planted.swift:1:' "$tmp/case.out"; then
      echo "self-test: '$name' failed without a file:line finding" >&2; cat "$tmp/case.out" >&2; rc=1
    fi
  done

  # A file importing both modules may not use PlanItem unqualified; a qualified use is fine.
  fresh
  printf 'import CapstanAppCore\nimport CapstanKit\nlet a: PlanItem? = nil\n' > "$tmp/repo/apps/macos/Capstan/Planted.swift"
  if run_checks "$tmp/repo" >"$tmp/amb.out" 2>&1; then
    echo "self-test: an ambiguous type name was not caught" >&2; rc=1
  elif ! grep -q 'Planted.swift:3: \[ambiguous\]' "$tmp/amb.out"; then
    echo "self-test: the ambiguous type name failed without a file:line finding" >&2; cat "$tmp/amb.out" >&2; rc=1
  fi
  printf 'import CapstanAppCore\nimport CapstanKit\nlet a: CapstanAppCore.PlanItem? = nil\n' > "$tmp/repo/apps/macos/Capstan/Planted.swift"
  if ! run_checks "$tmp/repo" >"$tmp/amb.out" 2>&1; then
    echo "self-test: a qualified type name was flagged" >&2; cat "$tmp/amb.out" >&2; rc=1
  fi

  # An allowed file may open links; a test file may hold a path.
  fresh
  mkdir -p "$tmp/repo/apps/macos/Capstan/Inbox" "$tmp/repo/apps/macos/CapstanKit/Tests/CapstanKitTests"
  echo 'let a = NSWorkspace.shared.open(u)' > "$tmp/repo/apps/macos/Capstan/Inbox/DesignLinkConfirm.swift"
  echo 'let p = "/home/x"' > "$tmp/repo/apps/macos/CapstanKit/Tests/CapstanKitTests/Planted.swift"
  echo 'import CryptoKit' > "$tmp/repo/apps/macos/CapstanKit/Sources/CapstanKit/Planted.swift"
  if ! run_checks "$tmp/repo" >"$tmp/allow.out" 2>&1; then
    echo "self-test: an allowed file or a test file was flagged" >&2; cat "$tmp/allow.out" >&2; rc=1
  fi

  # Plain literals, verbatim, labelled arguments, look-alike names and a waived line are not findings.
  fresh
  mkdir -p "$tmp/repo/apps/macos/Capstan"
  cat > "$tmp/repo/apps/macos/Capstan/Planted.swift" <<'SWIFT'
let a = MyText(name)
let b = x.Text(name)
let c = Text("plain")
let d = Text(verbatim: name)
let e = Button(action: go) { }
let f = Toggle(isOn: $on) { }
let g = Text(name) // static-text
SWIFT
  if ! run_checks "$tmp/repo" >"$tmp/ok6.out" 2>&1; then
    echo "self-test: a rule 6 non-finding was flagged" >&2; cat "$tmp/ok6.out" >&2; rc=1
  fi

  # Rule 7: a marked stub that lacks a listed line fails; a listed file that is missing fails; an implemented
  # file (marker removed) is skipped.
  fresh
  mkdir -p "$tmp/repo/apps/macos/Capstan"
  printf '// capstan-stub\nstruct SidebarView: View {\n}\n' > "$tmp/repo/apps/macos/Capstan/SidebarView.swift"
  printf 'apps/macos/Capstan/SidebarView.swift\tstruct SidebarView: View {\napps/macos/Capstan/SidebarView.swift\t    let model: AppModel\n' \
    > "$tmp/repo/apps/macos/scripts/stubs.txt"
  if run_checks "$tmp/repo" >"$tmp/stub.out" 2>&1; then
    echo "self-test: a marked stub with a missing line was not caught" >&2; rc=1
  fi
  printf 'struct SidebarView: View {\n}\n' > "$tmp/repo/apps/macos/Capstan/SidebarView.swift"
  if ! run_checks "$tmp/repo" >"$tmp/stub.out" 2>&1; then
    echo "self-test: an implemented file was flagged" >&2; cat "$tmp/stub.out" >&2; rc=1
  fi
  rm "$tmp/repo/apps/macos/Capstan/SidebarView.swift"
  if run_checks "$tmp/repo" >"$tmp/stub.out" 2>&1; then
    echo "self-test: a listed but missing stub file was not caught" >&2; rc=1
  fi

  # check-xcodeproj.py: a failing stand-in fails the script, a passing one passes.
  fresh
  printf 'import sys\nsys.exit(1)\n' > "$tmp/repo/apps/macos/scripts/check-xcodeproj.py"
  if run_checks "$tmp/repo" >"$tmp/xc.out" 2>&1; then
    echo "self-test: a failing check-xcodeproj.py was not caught" >&2; rc=1
  fi
  printf 'import sys\nsys.exit(0)\n' > "$tmp/repo/apps/macos/scripts/check-xcodeproj.py"
  if ! run_checks "$tmp/repo" >"$tmp/xc.out" 2>&1; then
    echo "self-test: a passing check-xcodeproj.py was flagged" >&2; cat "$tmp/xc.out" >&2; rc=1
  fi

  # A tree without check-xcodeproj.py or stubs.txt passes.
  fresh
  rm -f "$tmp/repo/apps/macos/scripts/check-xcodeproj.py" "$tmp/repo/apps/macos/scripts/stubs.txt"
  if ! run_checks "$tmp/repo" >"$tmp/none.out" 2>&1; then
    echo "self-test: a tree without the optional files was flagged" >&2; cat "$tmp/none.out" >&2; rc=1
  fi

  # check-pm-fields.py: a field the reader decodes that the document and the fixtures lack is caught; a card the
  # overview does not reference is caught.
  fresh
  local events=$tmp/repo/apps/macos/CapstanKit/Sources/CapstanKit/Protocol/Events.swift
  sed -i 's|public var pmRecoveredBody: PmRecoveredBody? {|&\n        _ = try? raw.string("plantedField")|' "$events"
  if run_checks "$tmp/repo" >"$tmp/pm.out" 2>&1; then
    echo "self-test: a decoded field missing from the document and fixtures was not caught" >&2; rc=1
  elif ! grep -q 'plantedField' "$tmp/pm.out"; then
    echo "self-test: the missing PM field was caught without its name" >&2; cat "$tmp/pm.out" >&2; rc=1
  fi
  fresh
  sed -i 's/PmCardView/SomethingElse/g' "$tmp/repo/apps/macos/Capstan/Inbox/OverviewView.swift"
  if run_checks "$tmp/repo" >"$tmp/pm.out" 2>&1; then
    echo "self-test: an overview without the PM card was not caught" >&2; rc=1
  fi

  # the coordinator without pmStuck and a sidebar without SidebarText.pmStuck are each caught
  fresh
  sed -i 's/pmStuck/somethingElse/g' "$tmp/repo/apps/macos/Capstan/System/NotificationCoordinator.swift"
  if run_checks "$tmp/repo" >"$tmp/pm.out" 2>&1; then
    echo "self-test: a coordinator without pmStuck was not caught" >&2; rc=1
  fi
  fresh
  sed -i 's/SidebarText\.pmStuck/SidebarText.other/g' "$tmp/repo/apps/macos/Capstan/Navigation/SidebarView.swift"
  if run_checks "$tmp/repo" >"$tmp/pm.out" 2>&1; then
    echo "self-test: a sidebar without SidebarText.pmStuck was not caught" >&2; rc=1
  fi

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
