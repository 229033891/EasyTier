import { describe, expect, it } from 'vitest'
import { deriveTrayRunState, isRunFault } from './tray_run_state'

function snap(instanceId: string, running?: boolean, errorMsg?: string) {
  return { instanceId, running, errorMsg }
}

describe('isRunFault', () => {
  it('treats an unreachable client as a fault in normal mode', () => {
    expect(isRunFault('normal')).toBe(true)
  })

  it('treats an unreachable remote client as a fault (nothing local is manageable)', () => {
    expect(isRunFault('remote')).toBe(true)
  })

  it('only faults a service that is supposed to be running', () => {
    expect(isRunFault('service', 'Running')).toBe(true)
    expect(isRunFault('service', 'Stopped')).toBe(false)
    expect(isRunFault('service', 'NotInstalled')).toBe(false)
    expect(isRunFault('service', undefined)).toBe(false)
  })
})

describe('deriveTrayRunState', () => {
  it('stays idle when the client is up but nothing runs (the reported bug)', () => {
    expect(deriveTrayRunState({ clientRunning: true, clientFailureIsFault: true, instances: [] }))
      .toEqual({ state: 'idle', runningCount: 0, failedCount: 0 })
  })

  it('is idle when the client is up and instances are known but stopped', () => {
    const result = deriveTrayRunState({
      clientRunning: true,
      clientFailureIsFault: true,
      instances: [snap('a', false)],
    })
    expect(result.state).toBe('idle')
    expect(result.failedCount).toBe(0)
  })

  it('is green once an instance runs, and counts it', () => {
    const result = deriveTrayRunState({
      clientRunning: true,
      clientFailureIsFault: true,
      instances: [snap('a', true)],
    })
    expect(result).toEqual({ state: 'running', runningCount: 1, failedCount: 0 })
  })

  it('counts an unprobed instance as running rather than reporting grey on a doubt', () => {
    const result = deriveTrayRunState({
      clientRunning: true,
      clientFailureIsFault: true,
      instances: [snap('a', true), snap('b')],
    })
    expect(result).toEqual({ state: 'running', runningCount: 2, failedCount: 0 })
  })

  it('flags a start failure even while a sibling is healthy', () => {
    const result = deriveTrayRunState({
      clientRunning: true,
      clientFailureIsFault: true,
      instances: [snap('a', true), snap('broken', false, 'tun device not found')],
    })
    expect(result).toEqual({ state: 'error', runningCount: 1, failedCount: 1, errorKind: 'instance' })
  })

  it('ignores a stopped instance with no error text', () => {
    const result = deriveTrayRunState({
      clientRunning: true,
      clientFailureIsFault: true,
      instances: [snap('a', true), snap('b', false, '   ')],
    })
    expect(result.state).toBe('running')
    expect(result.failedCount).toBe(0)
  })

  it('is red when the client is down and that is actionable', () => {
    expect(deriveTrayRunState({ clientRunning: false, clientFailureIsFault: true }))
      .toEqual({ state: 'error', runningCount: 0, failedCount: 0, errorKind: 'client' })
  })

  it('is grey when the client is down by design (service stopped / not installed)', () => {
    expect(deriveTrayRunState({ clientRunning: false, clientFailureIsFault: false }))
      .toEqual({ state: 'idle', runningCount: 0, failedCount: 0 })
  })

  it('lets an unreachable core outrank an instance failure', () => {
    const result = deriveTrayRunState({
      clientRunning: false,
      clientFailureIsFault: true,
      instances: [snap('broken', false, 'no route')],
    })
    expect(result.state).toBe('error')
    expect(result.errorKind).toBe('client')
  })
})
