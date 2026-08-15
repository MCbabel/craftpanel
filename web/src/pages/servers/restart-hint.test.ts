import { describe, expect, it } from 'vitest'

import { updatesNeedRestart } from './restart-hint'

const current = [{ has_update: false }, { has_update: false }]
const stale = [{ has_update: false }, { has_update: true }]

describe('updatesNeedRestart', () => {
	it('stays quiet while the server is down', () => {
		expect(updatesNeedRestart(false, stale, true)).toBe(false)
	})

	it('stays quiet when nothing is out of date', () => {
		expect(updatesNeedRestart(true, current, false)).toBe(false)
	})

	it('warns when one item is out of date', () => {
		expect(updatesNeedRestart(true, stale, false)).toBe(true)
	})

	it('warns when only the modpack is out of date', () => {
		expect(updatesNeedRestart(true, current, true)).toBe(true)
	})

	it('warns even without a single item', () => {
		expect(updatesNeedRestart(true, [], true)).toBe(true)
	})
})
