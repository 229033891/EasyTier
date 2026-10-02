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
