import assert from "node:assert/strict";
import { isContainerQuiesced, publicIPv4, ruleTarget, sniMatchesHost, verifyForwardRouteRules } from "./m1-egress-helper.mjs";

assert.equal(isContainerQuiesced({ Running: false, Paused: false }), true);
assert.equal(isContainerQuiesced({ Running: true, Paused: true }), true);
assert.equal(isContainerQuiesced({ Running: true, Paused: false }), false);
assert.equal(isContainerQuiesced(undefined), false);

assert.equal(publicIPv4("8.8.8.8"), true);
for (const address of ["192.0.0.8", "192.0.0.9", "192.0.2.1", "192.31.196.1", "100.64.0.1", "198.19.1.1", "224.0.0.1", "255.255.255.255"]) {
  assert.equal(publicIPv4(address), false, `${address} must not be treated as public IPv4`);
}
assert.equal(publicIPv4("2001:4860:4860::8888"), false);

assert.deepEqual(
  ruleTarget('-A DOCKER-USER -m comment --comment "decoy -j DROP -g ACCEPT" -j ACCEPT'),
  { kind: "-j", target: "ACCEPT" },
);
assert.deepEqual(ruleTarget("-A DOCKER-USER -g CAPSTAN_CHAIN"), { kind: "-g", target: "CAPSTAN_CHAIN" });
verifyForwardRouteRules("-A FORWARD -j DOCKER-USER\n");
assert.throws(() => verifyForwardRouteRules("-A FORWARD -s 192.0.2.4 -j DOCKER-USER\n-A FORWARD -j ACCEPT\n"), /does not route/);
assert.throws(() => verifyForwardRouteRules("-A FORWARD -i docker0 -j DOCKER-USER\n"), /does not route/);
assert.throws(() => verifyForwardRouteRules("-A FORWARD -j ACCEPT\n-A FORWARD -j DOCKER-USER\n"), /bypass/);
assert.throws(() => verifyForwardRouteRules("-A FORWARD -j DOCKER-ISOLATION\n-A FORWARD -j DOCKER-USER\n"), /bypass/);
assert.throws(() => verifyForwardRouteRules("-A FORWARD -j DOCKER-FORWARD\n"), /does not route/);
assert.throws(() => ruleTarget('-A DOCKER-USER -m comment --comment "unterminated -j DROP'), /Malformed quoted/);

assert.equal(sniMatchesHost("CHATGPT.COM", "chatgpt.com"), true);
assert.equal(sniMatchesHost("chatgpt.com.evil", "chatgpt.com"), false);

console.log("PASS egress helper enforces quiescent cleanup, public IPv4, parsed rule targets, and case-insensitive SNI");
