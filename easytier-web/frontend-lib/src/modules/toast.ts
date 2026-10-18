/** Shared toast lifetimes (ms) for Web / GUI / Magisk config UI. */
export const TOAST_LIFE = {
  success: 2500,
  info: 2500,
  warn: 5000,
  error: 5000,
  /** Mode/RPC/service failures that need longer reading time. */
  severe: 10000,
} as const

export type ToastLifeKind = keyof typeof TOAST_LIFE

/**
 * Default cooldown (ms) between two identical *recurring* failure toasts.
 *
 * Poll/retry driven paths each hand-rolled this 30s window
 * (`usePollingList.ts`, `deviceArchive.ts`); it lives here now so a failure
 * that repeats every tick (or every render of a loop) is reported once per
 * window instead of once per attempt.
 */
export const TOAST_REPEAT_COOLDOWN_MS = 30_000

/** Cooldown gate returned by {@link createToastRepeatGate}. */
export interface ToastRepeatGate {
  /**
   * True when `key` may be reported at `now` (defaults to `Date.now()`).
   * Records the emission time when it returns true.
   */
  allow: (key: string, now?: number) => boolean
  /** Drop all recorded timestamps (teardown / tests). */
  reset: () => void
}

/**
 * Per-key cooldown gate for recurring toasts.
 *
 * Keys are caller-chosen (e.g. `poll:devices`), so unrelated failures never
 * suppress each other. Deliberately a pure state machine — no timers, no
 * globals — so callers can pass `now` in tests and get deterministic results.
 *
 * Keys must come from a *finite* set of literals: entries are never evicted, so a
 * key built from data (`${instanceId}:${error}`) would grow the Map forever.
 */
export function createToastRepeatGate(
  cooldownMs: number = TOAST_REPEAT_COOLDOWN_MS,
): ToastRepeatGate {
  const lastEmittedAt = new Map<string, number>()
  return {
    allow(key: string, now: number = Date.now()): boolean {
      const previous = lastEmittedAt.get(key)
      if (previous !== undefined && now - previous < cooldownMs) {
        return false
      }
      lastEmittedAt.set(key, now)
      return true
    },
    reset(): void {
      lastEmittedAt.clear()
    },
  }
}

/**
 * Emit `message` unless `key` was already reported inside the cooldown.
 *
 * Returns true when the toast was actually added, so callers (and tests) can
 * assert on suppression. Generic over the message type: the PrimeVue
 * `ToastService` is matched structurally, so this module needs no `primevue`
 * import and callers keep their own `toast.add` typing.
 */
export function addThrottledToast<M>(
  toast: { add: (message: M) => void },
  gate: ToastRepeatGate,
  key: string,
  message: M,
  now?: number,
): boolean {
  if (!gate.allow(key, now)) {
    return false
  }
  toast.add(message)
  return true
}
