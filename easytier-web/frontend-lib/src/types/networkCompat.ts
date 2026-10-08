import {
  Action as AclAction,
  ChainType as AclChainType,
  Protocol as AclProtocol,
} from '../generated/proto/acl'
import type { NetworkConfig } from './network'

const UINT64_MAX = (1n << 64n) - 1n

type JsonRecord = Record<string, unknown>

export function prepareNetworkConfigForProtoJson(config: NetworkConfig): NetworkConfig {
  const prepared = dropUnsupportedJsonValues(applyLegacyAclDefaults(config)) as NetworkConfig
  const record = prepared as JsonRecord
  normalizeLegacyOptionalUint64(record, 'instance_recv_bps_limit')
  return prepared
}

function applyLegacyAclDefaults(config: NetworkConfig): NetworkConfig {
  const acl = config.acl
  const aclV1 = acl?.acl_v1
  if (!Array.isArray(aclV1?.chains)) return config

  return {
    ...config,
    acl: {
      ...acl,
      acl_v1: {
        ...aclV1,
        chains: aclV1.chains.map((chain) => ({
          ...chain,
          chain_type: chain.chain_type ?? AclChainType.UnspecifiedChain,
          default_action: chain.default_action ?? AclAction.Allow,
          rules: (chain.rules ?? []).map((rule) => ({
            ...rule,
            protocol: rule.protocol ?? AclProtocol.Any,
            action: rule.action ?? AclAction.Allow,
          })),
        })),
      },
    },
  }
}

function dropUnsupportedJsonValues(value: unknown): unknown {
  if (value === undefined) return undefined
  if (typeof value === 'number' && !Number.isFinite(value)) return undefined
  // protobuf-ts 把 int64/uint64 读成 BigInt，而它的 JSON 表示是十进制字符串。
  // 消息实例被展开成普通对象后 BigInt 会漏出来，再喂回 fromJson 会抛
  // "Cannot parse JSON bigint"（例如 managed_credentials[].expiry_unix），
  // 所以这里统一转回字符串。
  if (typeof value === 'bigint') return value.toString()

  if (Array.isArray(value)) {
    return value.map(dropUnsupportedJsonValues).filter((v) => v !== undefined)
  }

  if (isJsonRecord(value)) {
    return Object.fromEntries(
      Object.entries(value)
        .map(([k, v]) => [k, dropUnsupportedJsonValues(v)])
        .filter(([, v]) => v !== undefined),
    )
  }

  return value
}

function isJsonRecord(value: unknown): value is JsonRecord {
  return typeof value === 'object' && value !== null
}

function normalizeLegacyOptionalUint64(obj: JsonRecord, key: string): void {
  const value = obj[key]
  if (typeof value !== 'string') return

  const trimmed = value.trim()
  if (!isPositiveUint64String(trimmed)) {
    delete obj[key]
    return
  }

  obj[key] = trimmed
}

function isPositiveUint64String(value: string): boolean {
  if (!/^\d+$/.test(value)) return false

  const n = BigInt(value)
  return n > 0n && n <= UINT64_MAX
}
