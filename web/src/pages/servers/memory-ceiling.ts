import type { MemoryUsage } from '@/api'

export const FALLBACK_CEILING_MIB = 16384

export type HostState = 'loading' | 'ready' | 'unavailable'

export interface HostCapacity {
	state: HostState
	assignableMib: number | null
}

export interface MemoryCeiling {
	max: number | null
	fallback: boolean
}

export function memoryCeiling(usage: MemoryUsage, host: HostCapacity): MemoryCeiling {
	const assignable = host.state === 'ready' ? (host.assignableMib ?? 0) : 0

	if (usage.limit_mib !== null) {
		const left = Math.max(0, usage.limit_mib - usage.allocated_mib)
		return { max: Math.max(left, assignable), fallback: false }
	}

	if (host.state === 'ready') return { max: assignable, fallback: false }
	if (host.state === 'loading') return { max: null, fallback: false }
	return { max: FALLBACK_CEILING_MIB, fallback: true }
}
