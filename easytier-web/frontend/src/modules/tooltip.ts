import type { Directive, DirectiveBinding } from 'vue'
import Tooltip from 'primevue/tooltip'

/** 悬停提示：出现稍缓、移开后多停留一会儿，不自动消失 */
export const tooltipDefaults = {
    showDelay: 250,
    hideDelay: 400,
    autoHide: false,
}

function mergeBinding(binding: DirectiveBinding): DirectiveBinding {
    const value = binding.value
    const merged =
        value !== null && typeof value === 'object' && !Array.isArray(value)
            ? { ...tooltipDefaults, ...value }
            : { ...tooltipDefaults, value }

    return {
        ...binding,
        value: merged,
    }
}

function invoke(hook: string, el: HTMLElement, binding?: DirectiveBinding, vnode?: unknown, prevVnode?: unknown) {
    const fn = (Tooltip as Record<string, unknown>)[hook]
    if (typeof fn !== 'function') {
        return
    }
    // PrimeVue BaseDirective hooks rely on being invoked as methods of the directive object.
    if (binding) {
        ;(fn as Function).call(Tooltip, el, mergeBinding(binding), vnode, prevVnode)
    } else {
        ;(fn as Function).call(Tooltip, el)
    }
}

/**
 * Wrap PrimeVue Tooltip so string bindings work and lifecycle hooks (incl. created) are forwarded.
 * Without `created`, BaseDirective never initializes `$pd` / instance state and hover may do nothing.
 */
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
