import assert from "node:assert/strict";
import { test } from "node:test";
import { isMissingNetwork } from "../src/runtime/role-runtime-manager.js";

const failure = (stderr: string, name = "cstan-net-a") =>
  new Error(`docker network inspect ${name} failed (1): ${stderr}`);

test("a missing network is recognized only from Docker's own wording for that network", () => {
  assert.equal(
    isMissingNetwork(
      failure("Error response from daemon: network cstan-net-a not found"),
      "cstan-net-a",
    ),
    true,
  );
  assert.equal(
    isMissingNetwork(
      failure("Error: No such network: cstan-net-a"),
      "cstan-net-a",
    ),
    true,
  );
  for (const stderr of [
    'context "remote": context not found',
    "404 page not found",
    "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
    "Error response from daemon: network cstan-net-b not found",
    "Error response from daemon: network cstan-net-a not found; retry later",
  ])
    assert.equal(
      isMissingNetwork(failure(stderr), "cstan-net-a"),
      false,
      stderr,
    );
  assert.equal(
    isMissingNetwork("network cstan-net-a not found", "cstan-net-a"),
    false,
  );
  assert.equal(
    isMissingNetwork(
      failure("Error response from daemon: network cstanxnet-a not found"),
      "cstan.net-a",
    ),
    false,
  );
});
