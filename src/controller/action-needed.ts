/** Which controller messages ask the PM for a step. A pure prefix classifier over the controller's own notice texts; everything else is informational. */

const ACTION_NEEDED_PATTERNS: readonly RegExp[] = [
  /^Plan \S+ (signed off|approved|needs attention|cancelled)\b/,
  /^Plan \S+ package \S+ (reviewed|cancelled)\b/,
  /^Integration \S+ is blocked by a merge conflict/,
  /^Operator proposal \S+ from /,
  /^Delivery problem\b/,
  /^Agent (stalled|blocked)\b/,
  /^Agent \S+ \(.*\) is lost\b/,
  /^Finding\b/,
];

export function controllerActionNeeded(body: string): boolean {
  return ACTION_NEEDED_PATTERNS.some((pattern) => pattern.test(body));
}
