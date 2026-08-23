import { describe, expect, it } from 'vitest'

import type { JavaMajorEntry, JavaRuntimeOverview } from '@/api'

import { busy, canFetch, canRemove, failureOf, pollDelay, standingOf } from './runtimes'

function entry(over: Partial<JavaMajorEntry> = {}): JavaMajorEntry {
	return {
		major: 21,
		fetchable: true,
		runtime: null,
		system: null,
		job: null,
		servers: 0,
		running: [],
		...over,
	}
}

function laid(): JavaMajorEntry['runtime'] {
	return {
		vendor: 'temurin',
		version: '21.0.12+7',
		path: '/var/lib/craftpanel/runtimes/java-21/bin/java',
		directory: '/var/lib/craftpanel/runtimes/java-21',
		size_bytes: 50_000_000,
		laid_at: '2026-08-22T10:00:00Z',
	}
}

function overview(majors: JavaMajorEntry[], architecture: string | null = 'x64'): JavaRuntimeOverview {
	return {
		auto_install: true,
		architecture,
		directory: '/var/lib/craftpanel/runtimes',
		total_bytes: 0,
		majors,
	}
}

describe('the runtimes page', () => {
	it('polls only while something is being fetched', () => {
		expect(pollDelay(overview([entry()]))).toBeNull()
		expect(
			pollDelay(
				overview([
					entry({
						job: {
							stage: 'downloading',
							running: true,
							done_bytes: 1,
							total_bytes: 2,
							share: 0.5,
							failure: null,
							failure_code: null,
						},
					}),
				]),
			),
		).toBeGreaterThan(0)
		expect(pollDelay(null)).toBeNull()
	})

	it('tells a runtime the panel laid down from one the machine already had', () => {
		expect(standingOf(entry())).toBe('absent')
		expect(standingOf(entry({ runtime: laid() }))).toBe('laid')
		expect(
			standingOf(
				entry({ system: { vendor: 'temurin', version: '21.0.4', path: '/usr/bin/java' } }),
			),
		).toBe('system')
	})

	it('offers neither button while a server is running on the runtime', () => {
		const live = entry({ runtime: laid(), servers: 2, running: ['survival'] })
		expect(canFetch(overview([live]), live)).toBe(false)
		expect(canRemove(live)).toBe(false)

		const idle = entry({ runtime: laid(), servers: 2, running: [] })
		expect(canFetch(overview([idle]), idle)).toBe(true)
		expect(canRemove(idle)).toBe(true)
	})

	it('offers nothing to fetch on a machine Adoptium does not build for', () => {
		const one = entry()
		expect(canFetch(overview([one], null), one)).toBe(false)
	})

	it('keeps a failure on show once the attempt has stopped, and not before', () => {
		const running = entry({
			job: {
				stage: 'downloading',
				running: true,
				done_bytes: 0,
				total_bytes: 0,
				share: 0,
				failure: null,
				failure_code: null,
			},
		})
		expect(busy(running)).toBe(true)
		expect(failureOf(running)).toBeNull()

		const failed = entry({
			job: {
				stage: 'downloading',
				running: false,
				done_bytes: 0,
				total_bytes: 0,
				share: 0,
				failure: 'Adoptium answered 503',
				failure_code: 'java_download_failed',
			},
		})
		expect(busy(failed)).toBe(false)
		expect(failureOf(failed)).toBe('Adoptium answered 503')
	})
})
