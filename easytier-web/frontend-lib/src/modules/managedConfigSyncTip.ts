/**
 * Suppress the "console config synced to this device" toast when it is only
 * the echo of a local Save that just reported up and came back via reconcile.
 *
 * RemoteManagement still reloads the editor on the event; only the host tip
 * (easytier-gui `index.vue`) should call {@link shouldSuppressSyncedOverwriteToast}.
 *
 * Call {@link noteLocalConfigSaved} **before** `await save_config` / `run_network`
 * (or immediately on the call site) so a fast reconcile echo cannot race past
 * the note. A failed save may suppress real overwrite tips up to the echo
 * budget — acceptable.
 *
 * ## Why a budget of 2 (not 1)
 *
 * One local save can emit **two** `managed_config_synced_overwrite` events on
 * the device before the UI settles:
 * 1. Console reconcile writeback (`on_managed_config_overwrite` /
 *    `on_managed_config_patched` in the managed-config manager).
 * 2. A later `AdoptRuntime` when Status/config polling (`STATUS_POLL_MS≈5s`)
 *    calls `get_config` and finds `runtime` fingerprint ≠ storage's
 *    `last_observed_runtime_fp` (hot-patch path may have persisted a **toml**
 *    fingerprint while overwrite persisted a **runtime** fingerprint).
 *
 * Consuming only once left emit #2 as a delayed duplicate toast. Budget 2
 * covers that pair inside the window; a third event (true other-admin push)
 * still toasts. Prefer revision/fingerprint on the event when core can ship it.
 */

/** Window after a local-save note during which echo tips may be skipped. */
export const SYNC_OVERWRITE_ECHO_SUPPRESS_MS = 12_000

/**
 * How many overwrite tips one local-save note may suppress inside the window.
 * See module doc (reconcile + AdoptRuntime).
 */
export const SYNC_OVERWRITE_ECHO_BUDGET = 2

type SaveNote = { at: number, pending: number }

const lastLocalSaveAt = new Map<string, SaveNote>()

/**
 * Record that this instance is about to be / was just saved locally.
 * Prefer calling **before** the save/run await so echoes cannot win the race.
 * Arms {@link SYNC_OVERWRITE_ECHO_BUDGET} suppressions for this instance.
 */
export function noteLocalConfigSaved(instanceId: string, now: number = Date.now()): void {
  const id = instanceId?.trim()
  if (!id)
    return
  lastLocalSaveAt.set(id, { at: now, pending: SYNC_OVERWRITE_ECHO_BUDGET })
}

/**
 * True when a synced-overwrite tip for `instanceId` should be skipped as a
 * self-save echo. Decrements the pending budget; clears the note when the
 * budget is exhausted or the window has elapsed (doc §2.3.1).
 */
export function shouldSuppressSyncedOverwriteToast(
  instanceId: string | undefined,
  now: number = Date.now(),
): boolean {
  const id = instanceId?.trim()
  if (!id)
    return false
  const note = lastLocalSaveAt.get(id)
  if (!note)
    return false
  if (now - note.at >= SYNC_OVERWRITE_ECHO_SUPPRESS_MS) {
    lastLocalSaveAt.delete(id)
    return false
  }
  note.pending -= 1
  if (note.pending <= 0)
    lastLocalSaveAt.delete(id)
  else
    lastLocalSaveAt.set(id, note)
  return true
}

/** Test / teardown helper — not part of the public product API. */
export function resetLocalConfigSaveNotes(): void {
  lastLocalSaveAt.clear()
}
