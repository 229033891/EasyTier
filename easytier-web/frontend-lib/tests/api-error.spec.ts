import { describe, expect, it } from 'vitest'
import {
  classifyApiError,
  extractApiErrorPayload,
  formatApiErrorDetail,
} from '../src/modules/utils'

describe('API error formatting', () => {
  it('extracts message/code from axios-style response bodies', () => {
    expect(
      extractApiErrorPayload({
        response: {
          data: {
            message: 'Timeout(Elapsed(()))',
            code: 'rpc_timeout',
          },
        },
      }),
    ).toEqual({
      message: 'Timeout(Elapsed(()))',
      code: 'rpc_timeout',
      current_config_revision: undefined,
    })
  })

  it('extracts current_config_revision on CAS conflicts', () => {
    expect(
      extractApiErrorPayload({
        response: {
          data: {
            message: 'revision conflict',
            code: 'managed_config_revision_conflict',
            current_config_revision: 'rev-9',
          },
        },
      }),
    ).toEqual({
      message: 'revision conflict',
      code: 'managed_config_revision_conflict',
      current_config_revision: 'rev-9',
    })
  })

  it('keeps managed_config_invalid validation text in formatApiErrorDetail', () => {
    const t = (key: string) => `i18n:${key}`
    expect(
      formatApiErrorDetail(
        {
          response: {
            data: {
              message: 'config_revision must not be empty',
              code: 'managed_config_invalid',
            },
          },
        },
        t,
      ),
    ).toBe('config_revision must not be empty')
    expect(
      classifyApiError({
        message: 'config_revision must not be empty',
        code: 'managed_config_invalid',
      }),
    ).toBe('validation')
  })

  it('unwraps JSON-string message blobs instead of showing raw JSON', () => {
    expect(
      extractApiErrorPayload({
        response: {
          data: '{"message":"Timeout(Elapsed(()))"}',
        },
      }),
    ).toEqual({
      message: 'Timeout(Elapsed(()))',
      code: undefined,
      current_config_revision: undefined,
    })
  })

  it('classifies timeout errors from code or legacy Debug text', () => {
    expect(
      classifyApiError({ message: 'Timeout: deadline has elapsed', code: 'rpc_timeout' }),
    ).toBe('timeout')
    expect(classifyApiError({ message: 'Timeout(Elapsed(()))' })).toBe('timeout')
    expect(classifyApiError({ message: 'RPC Error: Timeout(Elapsed(()))' })).toBe('timeout')
  })

  it('classifies unauthorized from stable code or Not authenticated text', () => {
    expect(classifyApiError({ message: 'Not authenticated', code: 'unauthorized' })).toBe(
      'unauthorized',
    )
    expect(classifyApiError({ message: 'Not authenticated' })).toBe('unauthorized')
    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'Not authenticated', code: 'unauthorized' } } },
        (key) => `i18n:${key}`,
      ),
    ).toBe('i18n:web.device_management.error_unauthorized')
  })

  it('maps known failures to localized copy', () => {
    const t = (key: string) => `i18n:${key}`

    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'Timeout(Elapsed(()))', code: 'rpc_timeout' } } },
        t,
      ),
    ).toBe('i18n:web.device_management.error_timeout')

    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'Client not found', code: 'client_not_found' } } },
        t,
      ),
    ).toBe('i18n:web.device_management.error_device_offline')

    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'custom backend detail' } } },
        t,
      ),
    ).toBe('custom backend detail')

    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'Database operation failed', code: 'db_error' } } },
        t,
      ),
    ).toBe('i18n:web.device_management.error_db')

    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'Tunnel error: reset', code: 'rpc_tunnel' } } },
        t,
      ),
    ).toBe('i18n:web.device_management.error_rpc_tunnel')

    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'Rust error: invalid listener', code: 'rpc_execution' } } },
        t,
      ),
    ).toBe('i18n:web.device_management.error_rpc_execution')

    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'Shutdown', code: 'rpc_shutdown' } } },
        t,
      ),
    ).toBe('i18n:web.device_management.error_rpc_shutdown')

    expect(
      formatApiErrorDetail(
        { response: { data: { message: 'Decode error', code: 'rpc_error' } } },
        t,
      ),
    ).toBe('i18n:web.device_management.error_rpc')
  })

  it('classifies legacy RPC display text without codes', () => {
    expect(classifyApiError({ message: 'Tunnel error: connection reset' })).toBe('rpc_tunnel')
    expect(classifyApiError({ message: 'Rust error: bad config' })).toBe('rpc_execution')
    expect(classifyApiError({ message: 'Shutdown' })).toBe('rpc_shutdown')
  })
})
