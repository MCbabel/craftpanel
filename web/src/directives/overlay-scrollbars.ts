import { OverlayScrollbars, type PartialOptions } from 'overlayscrollbars'
import type { ObjectDirective } from 'vue'

const defaults = Object.freeze<PartialOptions>({
	scrollbars: {
		theme: 'os-theme-dark',
		autoHide: 'leave',
		autoHideSuspend: true,
	},
})

const merge = (options: PartialOptions = {}): PartialOptions => ({
	...defaults,
	...options,
	scrollbars: { ...defaults.scrollbars, ...(options.scrollbars ?? {}) },
})

export const overlayScrollbarsDirective: ObjectDirective<HTMLElement, PartialOptions | undefined> =
	{
		mounted(el, binding) {
			OverlayScrollbars(el, merge(binding.value))
		},
		updated(el, binding) {
			if (binding.value === binding.oldValue) return
			OverlayScrollbars(el)?.options(merge(binding.value))
		},
		unmounted(el) {
			OverlayScrollbars(el)?.destroy()
		},
	}
