import { useMediaQuery } from '@vueuse/core'
import type { Ref } from 'vue'

export function useWideScreen(): Ref<boolean> {
	return useMediaQuery('(min-width: 768px)')
}
