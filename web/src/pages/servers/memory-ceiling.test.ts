import { describe, expect, it } from 'vitest'

import type { MemoryUsage } from '@/api'

import { FALLBACK_CEILING_MIB, type HostCapacity, memoryCeiling } from './memory-ceiling'

function usage(limitMib: number | null, allocatedMib: number): MemoryUsage {
	return { limit_mib: limitMib, allocated_mib: allocatedMib, used_bytes: 0 }
}

const loading: HostCapacity = { state: 'loading', assignableMib: null }
const unavailable: HostCapacity = { state: 'unavailable', assignableMib: null }
const ready: HostCapacity = { state: 'ready', assignableMib: 8192 }

describe('memoryCeiling', () => {
	it('measures an account with a budget by what its budget still gives', () => {
		expect(memoryCeiling(usage(4096, 1024), loading)).toEqual({ max: 3072, fallback: false })
	})

	it('lets an account with a budget into the machine once the number is there', () => {
		expect(memoryCeiling(usage(4096, 1024), ready)).toEqual({ max: 8192, fallback: false })
	})

	it('reaches zero on an exhausted budget and never goes below it', () => {
		expect(memoryCeiling(usage(2048, 4096), loading)).toEqual({ max: 0, fallback: false })
	})

	it('takes the machine for an account without a budget', () => {
		expect(memoryCeiling(usage(null, 2048), ready)).toEqual({ max: 8192, fallback: false })
	})

	it('waits for the machine figure instead of flashing up "no memory left"', () => {
		expect(memoryCeiling(usage(null, 0), loading)).toEqual({ max: null, fallback: false })
	})

	it('gives an account without a budget a stand-in ceiling when the machine figure fails', () => {
		const ceiling = memoryCeiling(usage(null, 0), unavailable)

		expect(ceiling).toEqual({ max: FALLBACK_CEILING_MIB, fallback: true })
		expect(ceiling.max).toBeGreaterThanOrEqual(512)
	})

	it('names no stand-in ceiling for an account with a budget — its budget is the real figure', () => {
		expect(memoryCeiling(usage(4096, 1024), unavailable)).toEqual({ max: 3072, fallback: false })
	})
})
