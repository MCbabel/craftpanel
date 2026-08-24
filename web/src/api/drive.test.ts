import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ApiRequestError } from './client'
import {
	drive,
	type DriveLink,
	type DriveStatus,
	linkPhase,
	noLinkOpen,
	statusPollMs,
	dayLeft,
	dayShare,
	storageLeft,
	storageShare,
} from './drive'

const fetchMock = vi.fn<typeof fetch>()

function replies(status: number, body: string | null): typeof fetchMock {
	return fetchMock.mockImplementation(() =>
		Promise.resolve(
			new Response(body, { status, headers: { 'content-type': 'application/json' } }),
		),
	)
}

beforeEach(() => {
	fetchMock.mockReset()
	vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
	vi.unstubAllGlobals()
})

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
		started_at: '2026-08-13T10:00:00Z',
		expires_at: '2026-08-13T10:30:00Z',
		interval: 5,
		...over,
	}
}

describe('The calls from section 22', () => {
	it('disconnect with ?files= and without — and without is the question, not the answer (22.7)', async () => {
		replies(204, null)
		await drive.disconnect('keep')
		expect(fetchMock.mock.calls[0]?.[0]).toContain('/drive?files=keep')

		fetchMock.mockClear()
		await drive.disconnect('delete')
		expect(fetchMock.mock.calls[0]?.[0]).toContain('/drive?files=delete')

		fetchMock.mockClear()
		await drive.disconnect()
		expect(fetchMock.mock.calls[0]?.[0]).not.toContain('files=')
	})

	it('sends no `client_secret` when saving if it stays unchanged (22.12)', async () => {
		replies(200, '{"configured":true,"client_id":"a","target_policy":"user_choice","folder_name":"f","accounts":[]}')

		await drive.save({
			client_id: 'a',
			target_policy: 'drive_only',
			folder_name: 'craftpanel-backups',
		})

		const body = String(fetchMock.mock.calls[0]?.[1]?.body ?? '')
		expect(body).not.toContain('client_secret')
		expect(JSON.parse(body)).toEqual({
			client_id: 'a',
			target_policy: 'drive_only',
			folder_name: 'craftpanel-backups',
		})
	})

	it('knows no ?files= when disconnecting somebody else\'s account (22.14)', async () => {
		replies(204, null)
		await drive.disconnectUser('01JZ8Q9V0RS2H5PT7YF3D1XKAE')

		const [url, init] = fetchMock.mock.calls[0] ?? []
		expect(String(url)).toContain('/admin/drive/01JZ8Q9V0RS2H5PT7YF3D1XKAE')
		expect(String(url)).not.toContain('files=')
		expect(init?.method).toBe('DELETE')
	})

	it('passes the error code out of the envelope of 1.7 through', async () => {
		replies(409, '{"error":"drive_not_configured","message":"nothing set up"}')

		await expect(drive.startLink()).rejects.toMatchObject({
			status: 409,
			code: 'drive_not_configured',
			message: 'nothing set up',
		})
	})
})

describe('The run in progress', () => {
	it('reads the deadline as Google\'s own and not as ours (22.4)', () => {
		const now = Date.parse('2026-08-13T10:10:00Z')
		expect(linkPhase(link(), now)).toBe('waiting')
		expect(linkPhase(link(), Date.parse('2026-08-13T10:30:01Z'))).toBe('expired')
		expect(linkPhase(link({ state: 'denied' }), now)).toBe('denied')
		expect(linkPhase(link({ state: 'accepted' }), now)).toBe('accepted')
	})

	it('leaves a run with an unreadable deadline open instead of clearing it away', () => {
		expect(linkPhase(link({ expires_at: 'some time' }), Date.now())).toBe('waiting')
	})

	it('reads a 404 as "no run open" and not as a failure (22.5)', () => {
		expect(noLinkOpen(new ApiRequestError(404, 'drive_link_not_found', 'nothing open'))).toBe(true)
		expect(noLinkOpen(new ApiRequestError(502, 'drive_unavailable', 'Google is down'))).toBe(false)
		expect(noLinkOpen(new Error('the network went away'))).toBe(false)
	})
})

describe('How often it looks again', () => {
	it('does not ask at all while the operator has set nothing up (22.2)', () => {
		expect(statusPollMs(null)).toBeNull()
		expect(statusPollMs(status({ panel_configured: false }))).toBeNull()
	})

	it('asks at the rate Google names while a code is open', () => {
		const running = status({ configured: false, link: link({ interval: 7 }) })
		expect(statusPollMs(running, Date.parse('2026-08-13T10:10:00Z'))).toBe(7_000)
		expect(statusPollMs(running, Date.parse('2026-08-13T11:00:00Z'))).toBeNull()
	})

	it('stops when nothing is connected and nothing is running', () => {
		expect(statusPollMs(status({ configured: false }))).toBeNull()
		expect(statusPollMs(status())).toBe(30_000)
	})
})

describe('The day\u2019s share of Google', () => {
	it('reads as empty on an account that has sent nothing today', () => {
		expect(dayShare(status())).toBe(0)
		expect(dayLeft(status())).toBe(750_000_000_000)
	})

	it('works out how much of the 750 GB is gone', () => {
		const half = status({ uploaded_today_bytes: 375_000_000_000 })
		expect(dayShare(half)).toBeCloseTo(0.5, 6)
		expect(dayLeft(half)).toBe(375_000_000_000)
	})

	it('does not let a day that went over read as more than spent', () => {
		const over = status({ uploaded_today_bytes: 900_000_000_000 })
		expect(dayShare(over)).toBe(1)
		expect(dayLeft(over)).toBe(0)
	})
})

describe('The storage figure', () => {
	it('reads null as unlimited and not as full (22.3)', () => {
		expect(storageLeft(status({ storage_limit_bytes: null }))).toBeNull()
		expect(storageShare(status({ storage_limit_bytes: null }))).toBeNull()
	})

	it('works out what is left and the share from what Google names', () => {
		expect(storageLeft(status())).toBe(16_106_127_360 - 2_147_483_648)
		expect(storageShare(status())).toBeCloseTo(0.1333, 3)
	})

	it('does not let an overdrawn store go negative', () => {
		const over = status({ storage_limit_bytes: 100, storage_usage_bytes: 250 })
		expect(storageLeft(over)).toBe(0)
		expect(storageShare(over)).toBe(1)
	})
})
