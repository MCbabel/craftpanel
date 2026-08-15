import { describe, expect, it } from 'vitest'

import { gigabytes, mebibytes, MEMORY_MIN_GB, wholeGigabytes } from './memory-gb'

describe('memory-gb', () => {
	it('shows half gigabytes instead of rounding them away', () => {
		expect(gigabytes(11776)).toBe(11.5)
		expect(gigabytes(2048)).toBe(2)
		expect(gigabytes(512)).toBe(0.5)
	})

	it('comes back out of the display unchanged', () => {
		for (const mib of [1024, 2048, 11776, 22528]) {
			expect(mebibytes(gigabytes(mib))).toBe(mib)
		}
	})

	it('rounds the upper end down — an end the knob cannot reach would be a lie', () => {
		expect(wholeGigabytes(22528)).toBe(22)
		expect(wholeGigabytes(2560)).toBe(2)
		expect(wholeGigabytes(3000)).toBe(2)
	})

	it('drops below the slider floor when less than a gigabyte of budget is left', () => {
		expect(wholeGigabytes(512)).toBeLessThan(MEMORY_MIN_GB)
		expect(wholeGigabytes(0)).toBeLessThan(MEMORY_MIN_GB)
		expect(wholeGigabytes(1024)).toBe(MEMORY_MIN_GB)
	})

	it('keeps its smallest step above what the core still accepts', () => {
		expect(mebibytes(MEMORY_MIN_GB)).toBeGreaterThanOrEqual(512)
	})
})
