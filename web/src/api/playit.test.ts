import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { isApiRequestError } from './client'
import {
	type PlayitClaim,
	claimPhase,
	notFound,
	otherAddresses,
	playit,
	playitAbsent,
	type PlayitAddress,
	type PlayitClaimState,
	type PlayitOverview,
	type PlayitStatus,
	portsLeft,
	publicAddress,
	shareAddress,
	type ServerTunnel,
	statusPollMs,
	tunnelAddresses,
	tunnelPollMs,
} from './playit'

const fetchMock = vi.fn<typeof fetch>()

function replies(status: number, body: string | null, type = 'application/json'): typeof fetchMock {
	return fetchMock.mockImplementation(() =>
		Promise.resolve(new Response(body, { status, headers: { 'content-type': type } })),
	)
}

beforeEach(() => {
	fetchMock.mockReset()
	vi.stubGlobal('fetch', fetchMock)
})

afterEach(() => {
	vi.unstubAllGlobals()
})

function tunnel(over: Partial<ServerTunnel> = {}): ServerTunnel {
	return {
		state: 'online',
		addresses: [],
		local_port: 25565,
		detail: null,
		created_at: '2026-08-13T10:00:00Z',
		checked_at: '2026-08-13T10:00:30Z',
		...over,
	}
}

const AUTO: PlayitAddress = { address: 'olive-hound.gl.at.ply.gg', kind: 'auto' }
const IP4: PlayitAddress = { address: '69.9.186.255:31427', kind: 'ip4' }

function status(over: Partial<PlayitStatus> = {}): PlayitStatus {
	return {
		configured: true,
		agent_id: '2f8d1c8e-0f3f-4a1e-9e1a-0d0c0b0a0908',
		account_status: 'guest',
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

function overview(over: Partial<PlayitOverview> = {}): PlayitOverview {
	return {
		user_id: '01J000000000000000000ANN',
		username: 'anna',
		configured: true,
		account_status: 'verified',
		is_self_managed: true,
		has_premium: false,
		agent: { state: 'running', version: '1.0.10', detail: null },
		ports: { used: 1, limit: 4, for_others: 0 },
		last_error: null,
		checked_at: '2026-08-13T10:00:30Z',
		...over,
	}
}

describe('Addresses (PLAYIT.md 2.4, 10.2)', () => {
	it('keeps playit\'s order and throws none away', () => {
		const ipv6: PlayitAddress = { address: '[2606:4700::1]:31427', kind: 'ip6' }
		const as_playit = tunnelAddresses(tunnel({ addresses: [AUTO, IP4, ipv6] }))

		expect(as_playit.map((entry) => entry.kind)).toEqual(['auto', 'ip4', 'ip6'])
		expect(as_playit).toHaveLength(3)
	})

	it('reorders nothing, so a name of one\'s own from the paid plan stays in front', () => {
		const own: PlayitAddress = { address: 'mc.example.com', kind: 'domain' }
		const ordered = tunnelAddresses(tunnel({ addresses: [own, AUTO] }))

		expect(ordered.map((entry) => entry.kind)).toEqual(['domain', 'auto'])
	})

	it('puts nothing together, but passes playit\'s strings on unchanged', () => {
		expect(tunnelAddresses(tunnel({ addresses: [AUTO] }))[0]?.address).toBe(AUTO.address)
	})

	it('gets by with a server without a tunnel', () => {
		expect(tunnelAddresses(null)).toEqual([])
		expect(tunnelAddresses(tunnel({ state: 'none' }))).toEqual([])
	})
})

describe('Which address one passes on', () => {
	const REAL: PlayitAddress[] = [
		{ address: 'mauritania-nice.tun.ply.gg', kind: 'auto' },
		{ address: '231.ip.gl.ply.gg:15878', kind: 'auto' },
		{ address: '147.185.221.231:15878', kind: 'addr4' },
		{ address: '[2602:fbaf:0:1::e7]:15878', kind: 'addr6' },
	]

	it('takes playit\'s first address', () => {
		expect(shareAddress(tunnel({ addresses: REAL }))?.address).toBe('mauritania-nice.tun.ply.gg')
	})

	it('holds the rest next to it, in playit\'s order', () => {
		expect(otherAddresses(tunnel({ addresses: REAL })).map((e) => e.address)).toEqual([
			'231.ip.gl.ply.gg:15878',
			'147.185.221.231:15878',
			'[2602:fbaf:0:1::e7]:15878',
		])
	})

	it('names the same address in the server header', () => {
		expect(publicAddress(tunnel({ addresses: REAL }))).toBe('mauritania-nice.tun.ply.gg')
	})

	it('leaves a server without a tunnel without an address', () => {
		expect(shareAddress(null)).toBeNull()
		expect(otherAddresses(null)).toEqual([])
	})
})

describe('The address in the server header (PLAYIT.md 10.3)', () => {
	it('takes playit\'s first address for a tunnel that carries', () => {
		expect(publicAddress(tunnel({ addresses: [AUTO, IP4] }))).toBe(AUTO.address)
	})

	it('leaves the local address standing in every other state', () => {
		for (const state of ['pending', 'offline', 'missing', 'failed', 'none'] as const) {
			expect(publicAddress(tunnel({ state, addresses: [AUTO, IP4] }))).toBeNull()
		}
	})

	it('leaves it standing too when a tunnel is online but without an address', () => {
		expect(publicAddress(tunnel({ addresses: [] }))).toBeNull()
		expect(publicAddress(null)).toBeNull()
	})
})

describe('The claim (PLAYIT.md 1.4, 1.6)', () => {
	const started = '2026-08-13T10:00:00Z'
	const expires = '2026-08-13T10:15:00Z'
	const url = 'https://playit.gg/claim/34ddf358a8'

	function claim(state: PlayitClaimState): PlayitClaim {
		return { code: '34ddf358a8', url, state, started_at: started, expires_at: expires }
	}

	it('treats both waiting states alike', () => {
		const early = Date.parse('2026-08-13T10:01:00Z')

		expect(claimPhase(claim('waiting_for_visit'), early)).toBe('waiting')
		expect(claimPhase(claim('waiting_for_user'), early)).toBe('waiting')
	})

	it('knows consent and refusal', () => {
		const early = Date.parse('2026-08-13T10:01:00Z')

		expect(claimPhase(claim('accepted'), early)).toBe('accepted')
		expect(claimPhase(claim('rejected'), early)).toBe('rejected')
	})

	it('expires by our own deadline', () => {
		expect(claimPhase(claim('waiting_for_user'), Date.parse(expires))).toBe('expired')
	})

	it('does not throw away a consent already given because of the deadline', () => {
		expect(claimPhase(claim('accepted'), Date.parse('2026-08-13T11:00:00Z'))).toBe('accepted')
	})

	it('goes on waiting when the deadline is unreadable', () => {
		const broken = { ...claim('waiting_for_visit'), expires_at: 'some time' }

		expect(claimPhase(broken, Date.parse('2026-08-13T10:01:00Z'))).toBe('waiting')
	})
})

describe('A 404 as a statement (PLAYIT.md 8.3, 8.7)', () => {
	it('recognises the envelope of the panel', async () => {
		replies(404, '{"error":"playit_claim_not_found","message":"No claim is running."}')

		const failure = await playit.claim().catch((cause: unknown) => cause)

		expect(notFound(failure)).toBe(true)
		expect(isApiRequestError(failure) && failure.code).toBe('playit_claim_not_found')
	})

	it('recognises the bare 404 of a panel without playit too', async () => {
		replies(404, 'not found', 'text/plain; charset=utf-8')

		const failure = await playit
			.tunnel('01J000000000000000000SRV')
			.catch((cause: unknown) => cause)

		expect(notFound(failure)).toBe(true)
		expect(playitAbsent(failure)).toBe(true)
		expect(isApiRequestError(failure) && failure.code).toBe('http_404')
	})

	it('separates "there is no playit here" from "there is no such server"', async () => {
		replies(404, '{"error":"server_not_found","message":"No such server."}')

		const failure = await playit
			.tunnel('01J000000000000000000SRV')
			.catch((cause: unknown) => cause)

		expect(notFound(failure)).toBe(true)
		expect(playitAbsent(failure)).toBe(false)
	})

	it('holds everything else to be a fault', async () => {
		replies(409, '{"error":"playit_port_limit","message":"No ports left."}')

		const failure = await playit
			.createTunnel('01J000000000000000000SRV')
			.catch((cause: unknown) => cause)

		expect(notFound(failure)).toBe(false)
		expect(isApiRequestError(failure) && failure.message).toBe('No ports left.')
	})
})

describe('The calls (PLAYIT.md 8.1-8.11)', () => {
	it('speaks to the paths of the contract and sends no body', async () => {
		replies(200, JSON.stringify(status()))

		await playit.status()
		await playit.startClaim()
		await playit.restartAgent()

		const calls = fetchMock.mock.calls.map(([url, init]) => [url, init?.method, init?.body])
		expect(calls).toEqual([
			['/api/v1/playit', 'GET', undefined],
			['/api/v1/playit/claim', 'POST', undefined],
			['/api/v1/playit/agent/restart', 'POST', undefined],
		])
	})

	it('does not put one\'s own calls under /admin', async () => {
		replies(200, JSON.stringify(status()))

		await playit.status()
		await playit.claim()
		await playit.cancelClaim()

		const paths = fetchMock.mock.calls.map(([url]) => String(url))
		expect(paths.some((path) => path.startsWith('/api/v1/admin'))).toBe(false)
		expect(paths).toEqual([
			'/api/v1/playit',
			'/api/v1/playit/claim',
			'/api/v1/playit/claim',
		])
	})

	it('hangs the decision about the tunnels on the query, not on a body', async () => {
		replies(204, null)

		await playit.disconnect('keep')
		await playit.disconnect()

		expect(fetchMock.mock.calls.map(([url]) => url)).toEqual([
			'/api/v1/playit?tunnels=keep',
			'/api/v1/playit',
		])
		expect(fetchMock.mock.calls.every(([, init]) => init?.method === 'DELETE')).toBe(true)
	})

	it('reads the overview and disconnects somebody else\'s account by their user id', async () => {
		replies(200, JSON.stringify([overview()]))
		const list = await playit.overview()
		expect(list[0]?.username).toBe('anna')
		expect(list[0]).not.toHaveProperty('claim')

		replies(204, null)
		await playit.disconnectUser('01J000000000000000000ANN', 'delete')
		await playit.disconnectUser('01J000000000000000000ANN')

		const calls = fetchMock.mock.calls.map(([url, init]) => [url, init?.method, init?.body])
		expect(calls).toEqual([
			['/api/v1/admin/playit', 'GET', undefined],
			['/api/v1/admin/playit/01J000000000000000000ANN?tunnels=delete', 'DELETE', undefined],
			['/api/v1/admin/playit/01J000000000000000000ANN', 'DELETE', undefined],
		])
	})

	it('encodes the server id in the path', async () => {
		replies(200, JSON.stringify(tunnel()))

		await playit.tunnel('01J/000')

		expect(fetchMock.mock.calls[0]?.[0]).toBe('/api/v1/servers/01J%2F000/playit')
	})

	it('takes no port — not even by accident (PLAYIT.md 2.3)', async () => {
		replies(202, JSON.stringify(tunnel({ state: 'pending' })))

		await playit.createTunnel('01J000000000000000000SRV')

		const [url, init] = fetchMock.mock.calls[0] ?? []
		expect(String(url)).toBe('/api/v1/servers/01J000000000000000000SRV/playit')
		expect(init?.body).toBeUndefined()
	})
})

describe('Rate and limits', () => {
	it('counts the free ports against the limit of the plan', () => {
		expect(portsLeft(status({ ports: { used: 1, limit: 4, for_others: 0 } }))).toBe(3)
		expect(portsLeft(status({ ports: { used: 4, limit: 4, for_others: 0 } }))).toBe(0)
		expect(portsLeft(status({ ports: { used: 5, limit: 4, for_others: 1 } }))).toBe(0)
	})

	it('looks again sooner while the agent is starting or the file is being fetched', () => {
		expect(statusPollMs(status({ agent: { state: 'starting', version: null, detail: null } }))).toBe(
			3_000,
		)
		expect(
			statusPollMs(
				status({ binary: { state: 'fetching', version: null, arch: 'x86_64', detail: null } }),
			),
		).toBe(3_000)
		expect(statusPollMs(status())).toBe(20_000)
	})

	it('stops asking where the endpoints do not exist', () => {
		expect(tunnelPollMs(null, false)).toBeNull()
		expect(tunnelPollMs(tunnel(), true)).toBe(30_000)
		expect(tunnelPollMs(tunnel({ state: 'pending' }), true)).toBe(2_000)
	})
})
