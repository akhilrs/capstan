#!/usr/bin/env bash
# Static guards for the macOS client sources. A guard, not a proof: it cannot show that the code compiles.
#   check-static.sh              check the tree this script lives in
#   check-static.sh --self-test  plant each pattern in a temporary copy and expect a finding
set -u

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

  # (1) the kit imports nothing but Foundation and Dispatch; the app core may also import the kit
  local dir allowed_imports
  for dir in "CapstanKit|Foundation|Dispatch" "CapstanAppCore|Foundation|Dispatch|CapstanKit"; do
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

  # (8) the project file check belongs to another package; run it when it exists
  if [ -f "$macos/scripts/check-xcodeproj.py" ]; then
    if ! out=$(python3 -I "$macos/scripts/check-xcodeproj.py" "$macos/Capstan.xcodeproj" 2>&1); then
      echo "apps/macos/scripts/check-xcodeproj.py:1: [xcodeproj] $(echo "$out" | head -n 1)"; found=1
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
    cp -R "$root/apps/macos" "$tmp/repo/apps/macos"
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

  # An allowed file may open links; a test file may hold a path.
  fresh
  mkdir -p "$tmp/repo/apps/macos/Capstan/Inbox" "$tmp/repo/apps/macos/CapstanKit/Tests/CapstanKitTests"
  echo 'let a = NSWorkspace.shared.open(u)' > "$tmp/repo/apps/macos/Capstan/Inbox/DesignLinkConfirm.swift"
  echo 'let p = "/home/x"' > "$tmp/repo/apps/macos/CapstanKit/Tests/CapstanKitTests/Planted.swift"
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
