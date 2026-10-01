import assert from "node:assert/strict";
import { test } from "node:test";
import {
  OBSERVE_MAX_BYTES,
  parseObserveLines,
  sanitizeScreen,
  stripTerminalSequences,
} from "../src/observe.js";

test("lines is ASCII digits without a leading zero, 1 to 120, or the default", () => {
  assert.equal(parseObserveLines(undefined), 40);
  for (const ok of ["1", "9", "10", "40", "120"])
    assert.equal(parseObserveLines(ok), Number(ok), ok);
  for (const bad of [
    "0",
    "00",
    "007",
    "-1",
    "+5",
    "121",
    "999",
    "1000",
    "40abc",
    " 40",
    "4 0",
    "4.0",
    "",
    "٤٠",
    "1e2",
    "0x10",
  ])
    assert.equal(parseObserveLines(bad), null, JSON.stringify(bad));
});

test("terminal sequences are removed: CSI, OSC with BEL or ST, DCS, PM, APC, the 8-bit forms and unfinished ones (an introducer with no terminator is removed alone)", () => {
  const esc = "\u001b";
  const cases: Array<[string, string]> = [
    [`${esc}[31mred${esc}[0m`, "red"],
    [`a${esc}[1;38;5;196mb${esc}[K`, "ab"],
    [`${esc}]0;window title\u0007text`, "text"],
    [`${esc}]8;;http://x${esc}\\link${esc}]8;;${esc}\\`, "link"],
    [`${esc}Pq#0;2;0;0;0${esc}\\after`, "after"],
    [`${esc}_Gi=1;payload${esc}\\after`, "after"],
    [`${esc}^privacy${esc}\\after`, "after"],
    [`\u009b31mred\u009b0m`, "red"],
    [`\u009d0;title\u009ctext`, "text"],
    [`\u0090q data\u009cafter`, "after"],
    [`text${esc}[3`, "text"],
    [`text${esc}]0;never ended`, "text0;never ended"],
    [`text${esc}Pq never ended`, "textq never ended"],
    [`${esc}=${esc}>x${esc}7${esc}8`, "x"],
    [`${esc}(Bplain`, "plain"],
    [`lone${esc}`, "lone"],
    // An introducer with no terminator is removed alone: the text after it stays.
    [`before${esc}Phidden${esc}[31m rest`, "beforehidden rest"],
    [`a${esc}_x b${esc}^y c`, "ax by c"],
    [
      `${esc}]0;title never ended and then more text`,
      "0;title never ended and then more text",
    ],
    [`x\u009dy\u0090z\u0098w\u009ev\u009fu`, "xyzwvu"],
    // An OSC broken off by another escape keeps its payload as visible text.
    [`${esc}]8;;http://x${esc}[0mshown`, "8;;http://xshown"],
  ];
  for (const [input, expected] of cases)
    assert.equal(
      stripTerminalSequences(input),
      expected,
      JSON.stringify(input),
    );
});

test("the screen becomes plain text: CR and CRLF fold to LF, control, bidi and zero-width characters become spaces, LF stays", () => {
  assert.equal(
    sanitizeScreen("one\r\ntwo\rthree\nfour"),
    "one\ntwo\nthree\nfour",
  );
  assert.equal(sanitizeScreen("a\u0007b‮c​d⁦e\u0000f"), "a b c d e f");
  assert.equal(sanitizeScreen("x y\u0085z"), "x\ny\nz");
  assert.equal(sanitizeScreen("\u001b[31m  padded  \u001b[0m\n\n"), "padded");
  assert.equal(sanitizeScreen(""), "");
  assert.equal(sanitizeScreen("\u001b[2J\u001b[H"), "");
});

test("a long screen keeps its end, is cut at a character boundary and says so", () => {
  const long = Array.from({ length: 2000 }, (_, i) => `line ${i} éé`).join(
    "\n",
  );
  const text = sanitizeScreen(long);
  assert.ok(Buffer.byteLength(text, "utf8") <= OBSERVE_MAX_BYTES);
  assert.ok(text.startsWith("[earlier text cut]\n"));
  assert.ok(text.endsWith("line 1999 éé"));
  assert.ok(text.isWellFormed());
  const family = "\u{1F468}‍\u{1F469}‍\u{1F467}";
  const emojis = sanitizeScreen(`${family}`.repeat(1200));
  assert.ok(Buffer.byteLength(emojis, "utf8") <= OBSERVE_MAX_BYTES);
  assert.ok(emojis.isWellFormed());
  const exact = "a".repeat(OBSERVE_MAX_BYTES);
  assert.equal(sanitizeScreen(exact), exact, "text at the limit is not cut");
  assert.ok(sanitizeScreen(`${exact}b`).startsWith("[earlier text cut]"));
});
