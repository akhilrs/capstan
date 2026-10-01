/**
 * Starting a review and finishing it: pick the reviewer role, spawn a fresh
 * reviewer on a worktree at the reported commit, record the review, and
 * release the reviewer afterwards.
 */
import type { CapstanConfig } from "./config/capstan-config.js";
import type { ControllerCore, ReviewRecord } from "./controller/core.js";
import { MAX_REVIEW_TEXT_BYTES } from "./controller/core.js";
import type { MutationContext } from "./controller/types.js";

export class ReviewRequestError extends Error {
  override readonly name = "ReviewRequestError";
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

export interface ReviewLauncher {
  spawn(
    roleName: string,
    options: { readonly baseSha: string },
  ): Promise<{ readonly agentId: string; readonly state: string }>;
  release(agentId: string): Promise<unknown>;
}

export interface ReviewDeps {
  readonly core: ControllerCore;
  readonly launcher: ReviewLauncher;
  readonly config: CapstanConfig;
  readonly commitExists: (sha: string) => Promise<boolean>;
  readonly context: (credential: string) => MutationContext;
  readonly log: (event: string, details: Record<string, unknown>) => void;
}

/** The role to review with: the one asked for, else `reviewer`, else the only Verifier role. */
export function chooseReviewerRole(
  config: CapstanConfig,
  requested: string | undefined,
): string {
  const verifiers = config.roles.filter((role) => role.kind === "Verifier");
  if (requested !== undefined) {
    if (!verifiers.some((role) => role.name === requested))
      throw new ReviewRequestError(
        "unknown_reviewer_role",
        `${requested} is not a Verifier role in the configuration`,
      );
    return requested;
  }
  if (verifiers.some((role) => role.name === "reviewer")) return "reviewer";
  if (verifiers.length === 1) return verifiers[0]!.name;
  throw new ReviewRequestError(
    "no_reviewer_role",
    verifiers.length === 0
      ? "the configuration has no Verifier role to review with"
      : "several Verifier roles exist and none is named reviewer; name one",
  );
}

/**
 * Normalizes what a reviewer wrote: line breaks become LF, control and format
 * characters other than LF become spaces, and the text is cut at a character
 * boundary within the limit, with a marker when something was cut.
 */
export function reviewText(text: string): string {
  const clean = text
    .replace(/\r\n?/g, "\n")
    .replace(
      /[\p{Cc}\p{Cf}\p{Cs}\p{Zl}\p{Zp}\p{Noncharacter_Code_Point}]/gu,
      (c) => (c === "\n" ? c : " "),
    )
    .trim();
  if (Buffer.byteLength(clean, "utf8") <= MAX_REVIEW_TEXT_BYTES) return clean;
  const marker = " [text cut]";
  const room = MAX_REVIEW_TEXT_BYTES - Buffer.byteLength(marker, "utf8");
  let out = "";
  let bytes = 0;
  for (const { segment } of new Intl.Segmenter(undefined, {
    granularity: "grapheme",
  }).segment(clean)) {
    const size = Buffer.byteLength(segment, "utf8");
    if (bytes + size > room) break;
    out += segment;
    bytes += size;
  }
  return `${out.trimEnd()}${marker}`;
}

/** Spawns a fresh reviewer at the reported commit and records the review. A failure after the spawn releases the reviewer. */
export async function requestReview(
  deps: ReviewDeps,
  input: {
    readonly reportId: string;
    readonly requestedRole: string | undefined;
    readonly pmCredential: string;
  },
): Promise<{ readonly review: ReviewRecord; readonly spawnState: string }> {
  const role = chooseReviewerRole(deps.config, input.requestedRole);
  const check = deps.core.checkReviewRequest(input.reportId, role);
  if (!(await deps.commitExists(check.commitSha)))
    throw new ReviewRequestError(
      "commit_missing",
      "the reported commit no longer exists in the repository",
    );
  const spawned = await deps.launcher.spawn(role, { baseSha: check.commitSha });
  try {
    const review = deps.core.beginReview(deps.context(input.pmCredential), {
      reportId: input.reportId,
      reviewerRole: role,
      reviewerAgentId: spawned.agentId,
    });
    return { review, spawnState: spawned.state };
  } catch (error) {
    await deps.launcher.release(spawned.agentId).catch((releaseError) =>
      deps.log("review_reviewer_not_released", {
        agentId: spawned.agentId,
        error: String(releaseError),
      }),
    );
    throw error;
  }
}

/** Releases the reviewer of a finished review once the reply has gone out. */
export function releaseReviewerLater(
  deps: Pick<ReviewDeps, "launcher" | "log">,
  review: ReviewRecord,
): void {
  setImmediate(() => {
    deps.launcher
      .release(review.reviewerAgentId)
      .then(() =>
        deps.log("review_reviewer_released", { reviewId: review.reviewId }),
      )
      .catch((error) =>
        deps.log("review_reviewer_not_released", {
          reviewId: review.reviewId,
          error: String(error),
        }),
      );
  });
}

/** At daemon start: finished reviews whose reviewer is still active (a crash after the verdict) get their reviewer released. */
export async function recoverReviews(
  deps: Pick<ReviewDeps, "core" | "launcher" | "log">,
  credential: string,
): Promise<void> {
  for (const review of deps.core.reviewsToRelease(credential)) {
    try {
      await deps.launcher.release(review.reviewerAgentId);
      deps.log("review_reviewer_released", { reviewId: review.reviewId });
    } catch (error) {
      deps.log("review_reviewer_not_released", {
        reviewId: review.reviewId,
        error: String(error),
      });
    }
  }
}
