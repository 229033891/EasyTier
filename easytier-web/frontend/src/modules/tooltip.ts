import type { Directive, DirectiveBinding } from 'vue'
import Tooltip from 'primevue/tooltip'

/** 悬停提示：出现稍缓、移开后多停留一会儿，不自动消失 */
export const tooltipDefaults = {
    showDelay: 250,
    hideDelay: 2000,
    autoHide: true,
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

function invoke(hook: string, el: HTMLElement, binding?: DirectiveBinding, vnode?: unknown) {
    const fn = (Tooltip as Record<string, unknown>)[hook]
    if (typeof fn !== 'function') {
        return
    }
    if (binding) {
        fn(el, mergeBinding(binding), vnode)
    } else {
        fn(el)
    }
}

export const tooltipDirective: Directive = {
    beforeMount(el, binding, vnode) {
        invoke('beforeMount', el, binding, vnode)
    },
    mounted(el, binding, vnode) {
        invoke('mounted', el, binding, vnode)
    },
    updated(el, binding, vnode) {
        invoke('updated', el, binding, vnode)
    },
    unmounted(el) {
        invoke('unmounted', el)
        invoke('beforeUnmount', el)
    },
}
