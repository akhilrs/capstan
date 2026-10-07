/**
 * A generated large ledger with the shape of a long-running live one (hundreds of ended agents, a PM with
 * about a thousand messages, dozens of plans with signoffs, a few hundred reports and reviews, twenty thousand
 * events). It holds no live data: every id, text and time comes from the counters below, so two runs build
 * the same rows and the committed goldens (`*.golden.json`, captured with the code before the daemon-profile
 * fixes by `export.mjs`) stay comparable.
 */
export const SHAPE = {
  agents: 500,
  seats: 60,
  pmMessages: 1046,
  otherMessages: 2300,
  plans: 23,
  reports: 200,
  reviews: 287,
  events: 20_000,
  integrations: 30,
};

const T0 = Date.parse("2026-03-01T00:00:00.000Z");
export const stamp = (n) => new Date(T0 + n * 60_000).toISOString();
const sha = (n) => n.toString(16).padStart(40, "0");
const hash64 = (n) => n.toString(16).padStart(64, "0");

/** Fills the database (an adapter Database) of a fresh project; the core that created it must be closed. */
export function seedLargeLedger(db, projectId) {
  const S = SHAPE;
  const operatorActorId = db
    .prepare(
      "SELECT actor_id FROM actors WHERE project_id = ? AND role = 'operator'",
    )
    .get(projectId).actor_id;
  const first = (table) =>
    db
      .prepare(
        `SELECT COALESCE(MAX(sequence), 0) AS n FROM ${table} WHERE project_id = ?`,
      )
      .get(projectId).n;
  const messageBase = first("messages");
  const eventBase = first("controller_events");
  db.pragma("foreign_keys = OFF");
  db.transaction(() => {
    const roles = [
      ["pm", "PM"],
      ["developer", "Developer"],
      ["reviewer", "Verifier"],
      ["supervisor", "Supervisor"],
    ];
    for (const [name, kind] of roles)
      db.prepare(
        "INSERT OR IGNORE INTO role_definitions VALUES (?, ?, ?, 'claude', ?, 'active', ?, ?)",
      ).run(projectId, name, kind, hash64(name.length), stamp(0), stamp(0));
    // seat-0 is the PM's, seat-1..6 are Supervisor seats, the rest Developer seats.
    const seatKind = (i) =>
      i === 0 ? "PM" : i <= 6 ? "Supervisor" : "Developer";
    for (let i = 0; i < S.seats; i++)
      db.prepare("INSERT INTO seats VALUES (?, ?, ?, ?, 'active', 0, ?)").run(
        projectId,
        `seat-${i}`,
        `seat ${i}`,
        seatKind(i),
        stamp(0),
      );
    // Every tenth agent is a Supervisor; agent 0 is the PM.
    const kindOf = (i) =>
      i === 0
        ? ["pm", "PM", "seat-0"]
        : i % 10 === 1
          ? ["supervisor", "Supervisor", `seat-${1 + (i % 6)}`]
          : ["developer", "Developer", `seat-${7 + (i % (S.seats - 7))}`];
    for (let i = 0; i < S.agents; i++) {
      const [role, kind, seat] = kindOf(i);
      // The newest agents are active: the PM and the last six.
      const active = i === 0 || i >= S.agents - 6;
      db.prepare(
        "INSERT INTO actors VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
      ).run(
        `actor-${i}`,
        projectId,
        `${role}-${i}`,
        kind,
        seat,
        hash64(1000 + i),
        active ? 1 : 0,
        stamp(i),
        active ? null : stamp(i + 60),
      );
      db.prepare(
        "INSERT INTO agents VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, ?, ?)",
      ).run(
        projectId,
        i === 0 ? "pm-1" : `${role}-${i}`,
        role,
        kind,
        seat,
        `actor-${i}`,
        active ? "active" : "ended",
        stamp(i + 30),
        stamp(i),
        active ? null : stamp(i + 60),
      );
    }
    const agentId = (i) => (i === 0 ? "pm-1" : `${kindOf(i)[0]}-${i}`);
    const insertMessage = db.prepare(
      `INSERT INTO messages(project_id, message_id, sequence, recipient_agent_id, recipient_generation, sender_actor_id,
         body, body_hash, state, state_version, queued_at, sent_at, acked_at, deferral_count, send_attempts, created_at, updated_at, action_needed)
       VALUES (?, ?, ?, ?, 1, ?, ?, ?, ?, 0, ?, ?, ?, 0, 1, ?, ?, 0)`,
    );
    let sequence = messageBase;
    const message = (recipient, sender, body, state, at) => {
      sequence += 1;
      const id = `message-${sequence}`;
      const acked = state === "acked" || state === "acked_late";
      insertMessage.run(
        projectId,
        id,
        sequence,
        recipient,
        sender,
        body,
        hash64(sequence),
        state,
        stamp(at),
        state === "queued" ? null : stamp(at + 1),
        acked ? stamp(at + 2) : null,
        stamp(at),
        stamp(at + 2),
      );
      return id;
    };
    const filler = (n) =>
      `[capstan message m-${n} from operator]\n` +
      `Work on package ${n % 17}: ${"read the plan, keep the change small, run the checks and report. ".repeat(10 + (n % 7))}`;
    for (let n = 0; n < S.pmMessages; n++)
      message(
        "pm-1",
        `actor-${1 + (n % (S.agents - 1))}`,
        filler(n),
        n === S.pmMessages - 1 ? "queued" : "acked",
        100 + n * 3,
      );
    for (let n = 0; n < S.otherMessages; n++) {
      const to = 1 + (n % (S.agents - 1));
      message(
        agentId(to),
        "actor-0",
        filler(n + 5000),
        to >= S.agents - 6 && n > S.otherMessages - 20 ? "sent" : "acked",
        200 + n * 2,
      );
    }
    // Routine supervision checks sit on recent messages, as they do on a live ledger.
    for (let k = 0; k < 40; k++) {
      const id = messageBase + S.pmMessages + S.otherMessages - k;
      db.prepare("INSERT INTO supervision_checks VALUES (?, ?, ?)").run(
        projectId,
        `message-${id}`,
        stamp(200 + (S.otherMessages - k) * 2),
      );
    }
    // The notices the report relay looks for: one per approved plan and one per signoff.
    const planMessage = (planId, lead, at) => {
      message(
        "pm-1",
        "actor-0",
        `Plan ${planId} ${lead}\n${"details ".repeat(40)}`,
        "acked",
        at,
      );
    };
    const insertReport = db.prepare(
      "INSERT INTO agent_reports VALUES (?, ?, ?, ?, 1, ?, ?, 'capstan/b', ?, ?, ?, '{}', ?, ?)",
    );
    for (let n = 0; n < S.reports; n++) {
      const owner = 2 + ((n * 7) % (S.agents - 8));
      const accepted = n % 5 !== 0;
      insertReport.run(
        projectId,
        `report-${n}`,
        n + 1,
        agentId(owner),
        `actor-${owner}`,
        sha(n + 1),
        `report ${n}`,
        accepted ? "accepted" : "rejected",
        accepted ? null : "no new commit",
        accepted ? `message-${100 + n}` : null,
        stamp(300 + n * 5),
      );
    }
    const insertReview = db.prepare(
      `INSERT INTO reviews VALUES (?, ?, ?, ?, ?, NULL, NULL, NULL, ?, ?, ?, ?, 'actor-0', 'reviewer', ?, ?, ?, ?, NULL, ?, ?, ?)`,
    );
    for (let n = 0; n < S.reviews; n++) {
      const report = n % S.reports;
      const owner = 2 + ((report * 7) % (S.agents - 8));
      insertReview.run(
        projectId,
        `review-${n}`,
        n + 1,
        1 + Math.floor(n / S.reports),
        `report-${report}`,
        sha(report + 1),
        sha(0),
        agentId(owner),
        `actor-${owner}`,
        agentId(3),
        "actor-3",
        n % 4 === 0 ? "findings" : "passed",
        `verdict ${n}`,
        `message-${200 + n}`,
        stamp(400 + n * 5),
        stamp(401 + n * 5),
      );
    }
    db.prepare(
      "INSERT INTO supervision_control VALUES (?, 0, 0, 'degraded', 0, NULL, NULL, NULL, NULL, 0, ?) ON CONFLICT(project_id) DO UPDATE SET health = 'degraded'",
    ).run(projectId, stamp(0));
    const insertIntegration = db.prepare(
      `INSERT INTO integrations(project_id, integration_id, sequence, base_sha, branch, requested_by, state, head_sha, created_at, completed_at)
       VALUES (?, ?, ?, ?, ?, 'operator', 'running', NULL, ?, NULL)`,
    );
    const mergeIntegration = db.prepare(
      "UPDATE integrations SET state = 'merged', head_sha = ?, completed_at = ? WHERE project_id = ? AND integration_id = ?",
    );
    for (let n = 0; n < S.integrations; n++) {
      insertIntegration.run(
        projectId,
        `int-${n}`,
        n + 1,
        sha(0),
        `integration/plan-${n}`,
        stamp(2000 + n),
      );
      for (let p = 0; p < 5; p++)
        db.prepare("INSERT INTO integration_reports VALUES (?, ?, ?, ?)").run(
          projectId,
          `int-${n}`,
          p + 1,
          `report-${(n * 5 + p) % S.reports}`,
        );
      mergeIntegration.run(sha(n + 1), stamp(2001 + n), projectId, `int-${n}`);
    }
    const insertPlan = db.prepare(
      "INSERT INTO plans VALUES (?, ?, ?, ?, 'normal', 'draft', 'pm-1', 'developer-2', 0, NULL, NULL, ?, ?, NULL)",
    );
    for (let n = 1; n <= S.plans; n++) {
      insertPlan.run(
        projectId,
        `plan-${n}`,
        n,
        `Plan number ${n}`,
        stamp(1000 + n),
        stamp(1001 + n),
      );
      // Plan 22 has no approval notice and plan 23 no signoff notice: the relay has two left to announce.
      const body = JSON.stringify({
        summary: `plan ${n}`,
        packages: [0, 1, 2].map((p) => ({
          id: `pkg-${p}`,
          title: `Package ${p}`,
          owns: [`src/p${p}`],
          estimate_hours: 1,
          acceptance: ["works"],
        })),
      });
      db.prepare(
        "INSERT INTO plan_revisions VALUES (?, ?, 1, ?, ?, ?, 'developer-2', 'actor-2', ?)",
      ).run(projectId, `plan-${n}`, sha(0), body, hash64(n), stamp(1000 + n));
      db.prepare(
        "UPDATE plans SET state = 'approved', current_revision = 1, approved_revision = 1 WHERE project_id = ? AND plan_id = ?",
      ).run(projectId, `plan-${n}`);
      if (n !== 22) planMessage(`plan-${n}`, "approved", 1000 + n);
      db.prepare(
        "INSERT INTO plan_signoffs VALUES (?, ?, ?, 'pm-1', 'ok', ?)",
      ).run(projectId, `plan-${n}`, `int-${n - 1}`, stamp(1100 + n));
      if (n !== 23)
        planMessage(
          `plan-${n}`,
          `signed off. Integration int-${n - 1} merged`,
          1100 + n,
        );
      for (let p = 0; p < 3; p++) {
        const owner = 2 + ((n * 3 + p) % (S.agents - 8));
        db.prepare(
          "INSERT INTO plan_packages VALUES (?, ?, ?, ?, ?, NULL, NULL)",
        ).run(
          projectId,
          `plan-${n}`,
          `pkg-${p}`,
          agentId(owner),
          stamp(1010 + n * 3 + p),
        );
      }
    }
    // The controller events: mostly agent and message transitions, one degraded run_control event the status reads.
    const insertEvent = db.prepare(
      "INSERT INTO controller_events VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, 1, ?, ?)",
    );
    for (let n = 1; n <= S.events; n++) {
      const run = n === 5000;
      insertEvent.run(
        projectId,
        eventBase + n,
        `event-${n}`,
        run ? "run_control" : n % 2 === 0 ? "message" : "agent",
        run ? projectId : `entity-${n % 700}`,
        "queued",
        run ? "degraded" : "sent",
        operatorActorId,
        `request-${n}`,
        run
          ? JSON.stringify({ details: { reason: "generated degrade" } })
          : JSON.stringify({ details: { n, text: "x".repeat(60) } }),
        stamp(10 + n),
      );
    }
  })();
  db.pragma("foreign_keys = ON");
}
