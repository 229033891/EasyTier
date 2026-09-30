import type { Directive, DirectiveBinding } from 'vue'
import FloatingVue, { vTooltip } from 'floating-vue'

/** 全站统一：出现 200ms、移开 100ms 后消失 */
export const tooltipDefaults = {
    showDelay: 200,
    hideDelay: 100,
    delay: { show: 200, hide: 100 },
} as const

const tooltipTheme = FloatingVue.options?.themes?.tooltip as
    | { delay?: { show?: number; hide?: number } }
    | undefined
if (tooltipTheme) {
    tooltipTheme.delay = { ...tooltipDefaults.delay }
}

type TooltipInput = string | number | boolean | null | undefined | Record<string, unknown>

/**
 * 归一化绑定：兼容字符串、PrimeVue `{ value, escape }`、floating-vue `{ content, html }`。
 * 空值关闭提示，避免残留原生 title 式快慢不一。
 */
export function normalizeTooltipValue(value: TooltipInput): unknown {
    if (value == null || value === false || value === '') {
        return null
    }
    if (typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') {
        return {
            content: String(value),
            delay: { ...tooltipDefaults.delay },
        }
    }
    if (typeof value === 'object' && !Array.isArray(value)) {
        const raw = value as Record<string, unknown>
        const content = raw.content ?? raw.value
        if (content == null || content === false || content === '') {
            return null
        }
        const html =
            typeof raw.html === 'boolean'
                ? raw.html
                : raw.escape === false
        const delay =
            raw.delay && typeof raw.delay === 'object'
                ? raw.delay
                : { ...tooltipDefaults.delay }
        return {
            ...raw,
            content,
            html: !!html,
            delay,
        }
    }
    return null
}

function mergeBinding(binding: DirectiveBinding): DirectiveBinding {
    return {
        ...binding,
        value: normalizeTooltipValue(binding.value as TooltipInput),
    }
}

function invoke(
    hook: string,
    el: HTMLElement,
    binding?: DirectiveBinding,
    vnode?: unknown,
    prevVnode?: unknown,
) {
    const fn = (vTooltip as Record<string, unknown>)[hook]
    if (typeof fn !== 'function') {
        return
    }
    if (binding) {
        ;(fn as Function).call(vTooltip, el, mergeBinding(binding), vnode, prevVnode)
    } else {
        ;(fn as Function).call(vTooltip, el)
    }
}

/** 统一 tooltip 指令（floating-vue，避免 PrimeVue tooltip 内存泄漏） */
export const tooltipDirective: Directive = {
    created(el, binding, vnode, prevVnode) {
        invoke('created', el, binding, vnode, prevVnode)
    },
    beforeMount(el, binding, vnode, prevVnode) {
        invoke('beforeMount', el, binding, vnode, prevVnode)
    },
    mounted(el, binding, vnode, prevVnode) {
        invoke('mounted', el, binding, vnode, prevVnode)
    },
    beforeUpdate(el, binding, vnode, prevVnode) {
        invoke('beforeUpdate', el, binding, vnode, prevVnode)
    },
    updated(el, binding, vnode, prevVnode) {
        invoke('updated', el, binding, vnode, prevVnode)
    },
    beforeUnmount(el, binding, vnode, prevVnode) {
        invoke('beforeUnmount', el, binding, vnode, prevVnode)
    },
    unmounted(el, binding, vnode, prevVnode) {
        invoke('unmounted', el, binding, vnode, prevVnode)
    },
}
