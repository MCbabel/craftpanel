import { describe, expect, it } from 'vitest'

import type { DriveLink, DriveStatus } from '@/api/drive'

import { driveView, linkCountdown, readableCode } from './drive'

function status(over: Partial<DriveStatus> = {}): DriveStatus {
	return {
		panel_configured: true,
		configured: true,
		state: 'connected',
		google_name: 'Anna Example',
		google_email: 'anna@example.com',
		folder_name: 'craftpanel-backups',
		storage_limit_bytes: 16_106_127_360,
		storage_usage_bytes: 2_147_483_648,
		uploaded_today_bytes: 0,
		daily_upload_limit_bytes: 750_000_000_000,
		link: null,
		last_error: null,
		checked_at: '2026-08-13T10:00:30Z',
		...over,
	}
}

function link(over: Partial<DriveLink> = {}): DriveLink {
	return {
		user_code: 'GQVQ-JKEC',
		verification_url: 'https://www.google.com/device',
		state: 'waiting',
		started_at: '2099-01-01T10:00:00Z',
		expires_at: '2099-01-01T10:30:00Z',
		interval: 5,
		...over,
	}
}

describe('The Drive section of the account page', () => {
	it('still loading: no state, no figures, no alarm', () => {
		const view = driveView(null)

		expect(view.stage).toBe('unavailable')
		expect(view.broken).toBe(false)
		expect(view.storage.share).toBeNull()
		expect(view.link).toBeNull()
	})

	it('says "the operator has set nothing up" and not "fault" (22.2)', () => {
		const view = driveView(status({ panel_configured: false, configured: false, state: null }))

		expect(view.stage).toBe('unavailable')
		expect(view.broken).toBe(false)
	})

	it('separates "nothing connected" from "a code is running" (22.4)', () => {
		expect(driveView(status({ configured: false, state: null })).stage).toBe('unconnected')

		const linking = driveView(status({ configured: false, state: null, link: link() }))
		expect(linking.stage).toBe('linking')
		expect(linking.link?.user_code).toBe('GQVQ-JKEC')
	})

	it('forgets an expired code instead of going on showing it', () => {
		const stale = link({ started_at: '2020-01-01T10:00:00Z', expires_at: '2020-01-01T10:30:00Z' })
		const view = driveView(status({ configured: false, state: null, link: stale }))

		expect(view.stage).toBe('unconnected')
		expect(view.link).toBeNull()
	})

	it('carries the day\u2019s bytes so a stopped backup has a figure behind it', () => {
		const busy = driveView(status({ uploaded_today_bytes: 675_000_000_000 }))

		expect(busy.day.sentBytes).toBe(675_000_000_000)
		expect(busy.day.limitBytes).toBe(750_000_000_000)
		expect(busy.day.closing).toBe(true)
		expect(busy.day.spent).toBe(false)
	})

	it('says the day is spent instead of leaving the bar at almost full', () => {
		const spent = driveView(status({ uploaded_today_bytes: 750_000_000_000 }))

		expect(spent.day.spent).toBe(true)
		expect(spent.day.freeBytes).toBe(0)
		expect(spent.day.share).toBe(1)
		expect(spent.broken).toBe(false)
	})

	it('shows no day at all while the page is still loading', () => {
		expect(driveView(null).day.spent).toBe(false)
		expect(driveView(null).day.limitBytes).toBe(0)
	})

	it('shows a withdrawn access as connected and broken', () => {
		const view = driveView(status({ state: 'revoked', last_error: 'Google says no' }))

		expect(view.stage).toBe('connected')
		expect(view.broken).toBe(true)
		expect(view.lastFailure).toBeNull()
	})

	it('passes on the reason of the last failed attempt (22.5)', () => {
		const denied = driveView(
			status({
				configured: false,
				state: null,
				link: link({ state: 'denied' }),
				last_error: 'Google turned the request down (access_denied). … Test users …',
			}),
		)

		expect(denied.stage).toBe('unconnected')
		expect(denied.broken).toBe(false)
		expect(denied.lastFailure).toContain('access_denied')
	})

	it('reads unlimited storage as unlimited and not as full', () => {
		const view = driveView(status({ storage_limit_bytes: null }))

		expect(view.storage.limitBytes).toBeNull()
		expect(view.storage.freeBytes).toBeNull()
		expect(view.storage.share).toBeNull()
		expect(view.storage.nearlyFull).toBe(false)
	})

	it('warns as soon as the Drive is nearly full — a backup is gigabytes', () => {
		const roomy = driveView(status({ storage_limit_bytes: 1000, storage_usage_bytes: 500 }))
		expect(roomy.storage.nearlyFull).toBe(false)

		const tight = driveView(status({ storage_limit_bytes: 1000, storage_usage_bytes: 910 }))
		expect(tight.storage.nearlyFull).toBe(true)
		expect(tight.storage.freeBytes).toBe(90)
	})
})

describe('The deadline of the run', () => {
	it('works out m:ss from what Google states', () => {
		const now = Date.parse('2099-01-01T10:28:15Z')
		const counted = linkCountdown(link(), now)

		expect(counted.remaining).toBe('1:45')
		expect(counted.progress).toBeCloseTo(0.941, 2)
	})

	it('never goes negative and never above one', () => {
		const after = linkCountdown(link(), Date.parse('2099-01-01T11:00:00Z'))
		expect(after.remaining).toBe('0:00')
		expect(after.progress).toBe(1)
	})

	it('clears nothing away on an unreadable time, but shows 0:00', () => {
		const broken = linkCountdown(link({ expires_at: 'some time' }), Date.now())
		expect(broken).toEqual({ remaining: '0:00', progress: 0 })
	})

	it('hands out the code the way Google has it typed in', () => {
		expect(readableCode(link({ user_code: '  GQVQ-JKEC \n' }))).toBe('GQVQ-JKEC')
	})
})
