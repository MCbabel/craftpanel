import { describe, expect, it } from 'vitest'

import type { PlayitTunnelState, ServerTunnel } from '@/api/playit'

import { publishedPort, tunnelHoldsPrimaryPort } from './playit-ports'

function tunnel(state: PlayitTunnelState, localPort: number | null = 25565): ServerTunnel {
	return {
		state,
		addresses: [],
		local_port: localPort,
		detail: null,
		created_at: null,
		checked_at: null,
	}
}

describe('tunnelHoldsPrimaryPort', () => {
	it('holds the port in every state but none', () => {
		for (const state of ['pending', 'online', 'offline', 'missing', 'failed'] as const) {
			expect(tunnelHoldsPrimaryPort(tunnel(state), true), state).toBe(true)
		}
	})

	it('lets go only where there really is no row', () => {
		expect(tunnelHoldsPrimaryPort(tunnel('none', null), true)).toBe(false)
		expect(tunnelHoldsPrimaryPort(null, true)).toBe(false)
	})

	it('locks nothing when this panel does not offer playit at all', () => {
		expect(tunnelHoldsPrimaryPort(tunnel('online'), false)).toBe(false)
	})
})

describe('publishedPort', () => {
	it('names the port the hole points at, not today\'s primary one', () => {
		expect(publishedPort(tunnel('online', 25570), 25565)).toBe(25570)
	})

	it('falls back to the primary one for as long as playit has named none', () => {
		expect(publishedPort(tunnel('pending', null), 25565)).toBe(25565)
		expect(publishedPort(null, 25565)).toBe(25565)
	})
})
