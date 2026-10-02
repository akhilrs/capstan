import assert from "node:assert/strict";
import { test } from "node:test";
import {
  HERDR_AGENT_NAME,
  MAX_ROLE_NAME_CHARS,
  herdrAgentName,
  projectDisplayName,
  projectSlug,
  workspaceLabel,
} from "../src/herdr/naming.js";

test("the display name is the configured name, else the directory name, cleaned and cut to 24 characters", () => {
  assert.equal(projectDisplayName("Acme Shop", "/work/x"), "Acme Shop");
  assert.equal(projectDisplayName(null, "/work/vwatch.pro"), "vwatch.pro");
  assert.equal(projectDisplayName("   ", "/work/capstan"), "capstan");
  assert.equal(projectDisplayName("a\nb\u0000c", "/x"), "abc");
  assert.equal(projectDisplayName("x".repeat(40), "/x"), "x".repeat(24));
  assert.equal(projectDisplayName(null, "/"), "capstan");
});

test("the slug is lower case a-z0-9 runs joined by one dash, starts with a letter and has at most ten characters", () => {
  assert.equal(projectSlug("Acme Shop"), "acme-shop");
  assert.equal(projectSlug("vwatch.pro"), "vwatch-pro");
  assert.equal(projectSlug("123 go"), "p123-go");
  assert.equal(projectSlug("a very long project name"), "a-very-lon");
  assert.equal(projectSlug("ab--------cd------------"), "ab-cd");
  assert.equal(projectSlug("日本語"), "p");
  assert.equal(projectSlug("Ünïcode"), "unicode");
  assert.equal(projectSlug("x-"), "x");
});

test("the Herdr name is <slug>-<id> and must fit Herdr's rule", () => {
  assert.equal(herdrAgentName("acme", "pm-1"), "acme-pm-1");
  assert.equal(
    herdrAgentName("a-very-lon", "developer-1234"),
    "a-very-lon-developer-1234",
  );
  assert.ok(HERDR_AGENT_NAME.test("a-very-lon-developer-1234"));
  assert.throws(() => herdrAgentName("acme", "x".repeat(28)), /does not fit/);
  assert.throws(() => herdrAgentName("acme", "Pm-1"), /does not fit/);
  assert.equal(MAX_ROLE_NAME_CHARS, 16);
  const longest = herdrAgentName("a".repeat(10), `${"r".repeat(16)}-9999`);
  assert.equal(longest.length, 32);
});

test("a workspace label is `<project> · <part>` and stays within 64 characters", () => {
  assert.equal(workspaceLabel("Acme Shop", "pm"), "Acme Shop · pm");
  assert.equal(workspaceLabel("x".repeat(24), "y".repeat(80)).length, 64);
});
