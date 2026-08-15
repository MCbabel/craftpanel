import { describe, expect, it } from 'vitest'

import type { PlayitClaim, PlayitStatus } from '@/api/playit'

import { claimCountdown, playitView } from './playit'

function status(over: Partial<PlayitStatus> = {}): PlayitStatus {
	return {
		configured: true,
		agent_id: '2f8d1c8e-0f3f-4a1e-9e1a-0d0c0b0a0908',
		account_status: 'verified',
		is_self_managed: true,
		has_premium: false,
		agent: { state: 'running', version: '1.0.10', detail: null },
		binary: { state: 'ready', version: '1.0.10', arch: 'x86_64', detail: null },
		ports: { used: 1, limit: 4, for_others: 0 },
		claim: null,
		last_error: null,
		checked_at: '2026-08-13T10:00:30Z',
		...over,
	}
}

describe('The section of the account page', () => {
	it('still loading: no account, no figures, no alarm', () => {
		const view = playitView(null)

		expect(view.stage).toBe('unconnected')
		expect(view.ports).toEqual({ used: 0, limit: 0, free: 0, forOthers: 0, full: false })
		expect(view.notSelfManaged).toBe(false)
	})

	it('no key means: he can connect one', () => {
		expect(playitView(status({ configured: false })).stage).toBe('unconnected')
	})

	it('connected without an address is quiet and no fault', () => {
		const view = playitView(
			status({
				ports: { used: 0, limit: 4, for_others: 0 },
				agent: { state: 'absent', version: null, detail: null },
			}),
		)

		expect(view.stage).toBe('quiet')
		expect(view.ports.free).toBe(4)
		expect(view.ports.full).toBe(false)
	})

	it('with an address it is live', () => {
		expect(playitView(status()).stage).toBe('live')
	})

	it('counts the ports against the limit of the account and notices when none is free', () => {
		const full = playitView(status({ ports: { used: 4, limit: 4, for_others: 2 } }))

		expect(full.ports.free).toBe(0)
		expect(full.ports.full).toBe(true)
		expect(full.ports.forOthers).toBe(2)

		const premium = playitView(status({ has_premium: true, ports: { used: 4, limit: 16, for_others: 0 } }))
		expect(premium.ports.free).toBe(12)
		expect(premium.ports.full).toBe(false)
	})

	it('warns that the agent may hold no tunnels only after the first sync', () => {
		expect(playitView(status({ is_self_managed: false, checked_at: null })).notSelfManaged).toBe(
			false,
		)
		expect(playitView(status({ is_self_managed: false })).notSelfManaged).toBe(true)
		expect(
			playitView(status({ configured: false, is_self_managed: false })).notSelfManaged,
		).toBe(false)
	})

	it('calls a guest account a guest account', () => {
		expect(playitView(status({ account_status: 'guest' })).guest).toBe(true)
		expect(playitView(status({ account_status: null })).guest).toBe(false)
	})
})

describe('The deadline of the claim (PLAYIT.md 1.6)', () => {
	const claim: PlayitClaim = {
		code: '34ddf358a8',
		url: 'https://playit.gg/claim/34ddf358a8',
		state: 'waiting_for_visit',
		started_at: '2026-08-13T10:00:00Z',
		expires_at: '2026-08-13T10:15:00Z',
	}

	it('works out the time left and the way there', () => {
		expect(claimCountdown(claim, Date.parse('2026-08-13T10:00:00Z'))).toEqual({
			remaining: '15:00',
			progress: 0,
		})

		const half = claimCountdown(claim, Date.parse('2026-08-13T10:07:30Z'))
		expect(half.remaining).toBe('7:30')
		expect(half.progress).toBeCloseTo(0.5)

		expect(claimCountdown(claim, Date.parse('2026-08-13T10:14:01Z')).remaining).toBe('0:59')
	})

	it('does not run into the negative and not past the end', () => {
		const over = claimCountdown(claim, Date.parse('2026-08-13T11:00:00Z'))

		expect(over.remaining).toBe('0:00')
		expect(over.progress).toBe(1)
	})

	it('makes no number out of nonsense', () => {
		const broken = claimCountdown({ ...claim, expires_at: 'the day after tomorrow' }, Date.now())
		expect(broken).toEqual({ remaining: '0:00', progress: 0 })

		const noStart = claimCountdown({ ...claim, started_at: 'some time' }, Date.parse('2026-08-13T10:10:00Z'))
		expect(noStart.remaining).toBe('5:00')
		expect(noStart.progress).toBe(0)
	})
})
