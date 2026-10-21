// AGENTS.md: the ledger under `docs/archive/` is frozen and large — "never read
// it whole; grep it for one number". A permission rule cannot say that: a
// `Read(docs/archive/**)` deny also covers Grep and every shell read of the
// path, so it blocked the grep the rule prescribes. This judges the Read tool
// alone, as a plain function so the tests can call it without an engine.

/** The most lines a ranged read of the archive may take: one entry and its neighbours. */
export const MAX_LINES = 400

const ARCHIVE = /(^|[\\/])docs[\\/]archive[\\/]/i

/**
 * Why a Read of `path` with `limit` is refused, or `undefined` when it may run:
 * a path under `docs/archive/` read without a `limit`, or with one past
 * {@link MAX_LINES}.
 */
export function refusal(path: string, limit: number | undefined): string | undefined {
  if (!ARCHIVE.test(path)) return undefined
  if (limit !== undefined && limit <= MAX_LINES) return undefined
  return (
    `docs/archive/ is the frozen ledger; never read it whole (AGENTS.md). ` +
    `Grep it for one entry — grep -n '^\\*\\*D123 —' docs/archive/ledger.md — ` +
    `then read that range with offset and a limit of at most ${MAX_LINES}.`
  )
}
