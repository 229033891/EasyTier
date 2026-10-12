import { describe, expect, it } from 'vitest'
import {
  classifyResidual,
  isMislabeledFormerWebGhost,
  shouldAutoStartAfterDisabledSave,
} from '../src/modules/config-residual'

describe('classifyResidual', () => {
  const sets = {
    runningIds: new Set(['run-only', 'disabled-run', 'normal']),
    dbInstIds: new Set(['disabled-run', 'normal', 'stopped']),
    dbDisabledInstIds: new Set(['disabled-run', 'stopped']),
  }

  it('marks device-only when running and absent from DB', () => {
    expect(classifyResidual('run-only', sets)).toBe('device_only')
  })

  it('marks db-disabled-running when DB disabled and still running', () => {
    expect(classifyResidual('disabled-run', sets)).toBe('db_disabled_running')
  })

  it('returns null for normal running enabled rows', () => {
    expect(classifyResidual('normal', sets)).toBeNull()
  })

  it('returns null for stopped DB rows', () => {
    expect(classifyResidual('stopped', sets)).toBeNull()
  })
})

describe('shouldAutoStartAfterDisabledSave', () => {
  it('starts only when pending+disabled and device was running', () => {
    expect(shouldAutoStartAfterDisabledSave(
      { action: 'pending', reason: 'disabled' },
      true,
    )).toBe(true)
  })

  it('does not start when device was stopped', () => {
    expect(shouldAutoStartAfterDisabledSave(
      { action: 'pending', reason: 'disabled' },
      false,
    )).toBe(false)
  })

  it('does not start for patched/offline results', () => {
    expect(shouldAutoStartAfterDisabledSave(
      { action: 'patched' },
      true,
    )).toBe(false)
    expect(shouldAutoStartAfterDisabledSave(
      { action: 'pending', reason: 'offline' },
      true,
    )).toBe(false)
  })
})

describe('isMislabeledFormerWebGhost', () => {
  it('flags user/legacy local rows proven Web on device and absent from W', () => {
    expect(isMislabeledFormerWebGhost({
      localSource: 'user',
      instanceId: 'ghost',
      consoleWebOwnedIds: new Set(['other']),
      deviceMetaSource: 2,
    })).toBe(true)
  })

  it('does not flag pure local drafts (device meta not Web)', () => {
    expect(isMislabeledFormerWebGhost({
      localSource: 'user',
      instanceId: 'draft',
      consoleWebOwnedIds: new Set(),
      deviceMetaSource: 1,
    })).toBe(false)
  })

  it('does not flag rows still in console W', () => {
    expect(isMislabeledFormerWebGhost({
      localSource: 'legacy',
      instanceId: 'still-web',
      consoleWebOwnedIds: new Set(['still-web']),
      deviceMetaSource: 2,
    })).toBe(false)
  })
})
