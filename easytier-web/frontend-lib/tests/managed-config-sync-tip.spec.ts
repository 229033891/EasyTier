import { describe, expect, it, beforeEach } from 'vitest'
import {
  SYNC_OVERWRITE_ECHO_BUDGET,
  SYNC_OVERWRITE_ECHO_SUPPRESS_MS,
  noteLocalConfigSaved,
  resetLocalConfigSaveNotes,
  shouldSuppressSyncedOverwriteToast,
} from '../src/modules/managedConfigSyncTip'

describe('managed config sync tip echo suppress', () => {
  beforeEach(() => {
    resetLocalConfigSaveNotes()
  })

  it('does not suppress without a prior local save', () => {
    expect(shouldSuppressSyncedOverwriteToast('abc', 1_000)).toBe(false)
  })

  it(`suppresses up to ${SYNC_OVERWRITE_ECHO_BUDGET} overwrite tips within the window`, () => {
    noteLocalConfigSaved('abc', 1_000)
    for (let i = 0; i < SYNC_OVERWRITE_ECHO_BUDGET; i++)
      expect(shouldSuppressSyncedOverwriteToast('abc', 1_000 + i)).toBe(true)
    // Next event in the same window is a real console push — must toast.
    expect(shouldSuppressSyncedOverwriteToast('abc', 1_000 + SYNC_OVERWRITE_ECHO_BUDGET)).toBe(false)
  })

  it('stops suppressing after the window elapses (and clears the note)', () => {
    noteLocalConfigSaved('abc', 1_000)
    expect(shouldSuppressSyncedOverwriteToast('abc', 1_000 + SYNC_OVERWRITE_ECHO_SUPPRESS_MS)).toBe(false)
    expect(shouldSuppressSyncedOverwriteToast('abc', 1_000 + SYNC_OVERWRITE_ECHO_SUPPRESS_MS + 1)).toBe(false)
  })

  it('keeps independent budgets per instance', () => {
    noteLocalConfigSaved('a', 0)
    noteLocalConfigSaved('b', 0)
    expect(shouldSuppressSyncedOverwriteToast('a', 100)).toBe(true)
    expect(shouldSuppressSyncedOverwriteToast('b', 100)).toBe(true)
    expect(shouldSuppressSyncedOverwriteToast('c', 100)).toBe(false)
  })

  it('ignores blank instance ids', () => {
    noteLocalConfigSaved('  ', 0)
    expect(shouldSuppressSyncedOverwriteToast('  ', 1)).toBe(false)
    expect(shouldSuppressSyncedOverwriteToast(undefined, 1)).toBe(false)
  })
})
