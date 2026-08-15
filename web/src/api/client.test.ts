import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
	api,
	ApiRequestError,
	buildUrl,
	hasErrorCode,
	isApiRequestError,
	setUnauthenticatedHandler,
} from './client'

const fetchMock = vi.fn<typeof fetch>()

function reply(status: number, body: string, headers: Record<string, string> = {}): Response {
	return new Response(body, { status, headers: { 'content-type': 'application/json', ...headers } })
}

beforeEach(() => {
	fetchMock.mockReset()
	vi.stubGlobal('fetch', fetchMock)
	setUnauthenticatedHandler(null)
})

afterEach(() => {
	vi.unstubAllGlobals()
})

describe('Error envelope', () => {
	it('turns {error, message} into an error with a code', async () => {
		fetchMock.mockResolvedValue(
			reply(409, '{ "error": "server_not_running", "message": "The server is not running." }'),
		)

		const failure = await api.console
			.sendCommand('01J000000000000000000SRV', { command: 'say hello' })
			.catch((e: unknown) => e)

		expect(isApiRequestError(failure)).toBe(true)
		if (!isApiRequestError(failure)) return
		expect(failure.code).toBe('server_not_running')
		expect(failure.status).toBe(409)
		expect(failure.message).toBe('The server is not running.')
		expect(hasErrorCode(failure, 'server_busy', 'server_not_running')).toBe(true)
		expect(hasErrorCode(failure, 'server_busy')).toBe(false)
	})

	it('reads Retry-After on a 429', async () => {
		fetchMock.mockResolvedValue(
			reply(429, '{"error":"rate_limited","message":"Too many commands."}', { 'retry-after': '7' }),
		)

		const failure = await api.console.clear('01J000000000000000000SRV').catch((e: unknown) => e)

		expect(failure).toBeInstanceOf(ApiRequestError)
		if (!isApiRequestError(failure)) return
		expect(failure.code).toBe('rate_limited')
		expect(failure.retryAfterSeconds).toBe(7)
	})

	it('invents no code when no envelope comes', async () => {
		fetchMock.mockResolvedValue(new Response('<html>bad gateway</html>', { status: 502 }))

		const failure = await api.servers.list().catch((e: unknown) => e)

		expect(isApiRequestError(failure)).toBe(true)
		if (!isApiRequestError(failure)) return
		expect(failure.code).toBe('http_502')
		expect(failure.status).toBe(502)
	})

	it('turns a network failure into a typed error', async () => {
		fetchMock.mockRejectedValue(new TypeError('Failed to fetch'))

		const failure = await api.servers.list().catch((e: unknown) => e)

		expect(isApiRequestError(failure)).toBe(true)
		if (!isApiRequestError(failure)) return
		expect(failure.code).toBe('network_unreachable')
		expect(failure.status).toBe(0)
	})
})

describe('401', () => {
	it('reports unauthenticated exactly once', async () => {
		const toLogin = vi.fn()
		setUnauthenticatedHandler(toLogin)
		fetchMock.mockResolvedValue(reply(401, '{"error":"unauthenticated","message":"No session."}'))

		await api.auth.me().catch(() => undefined)
		await api.auth.me().catch(() => undefined)

		expect(toLogin).toHaveBeenCalledTimes(1)
	})

	it('reports again after one request has succeeded', async () => {
		const toLogin = vi.fn()
		setUnauthenticatedHandler(toLogin)

		fetchMock.mockResolvedValueOnce(
			reply(401, '{"error":"unauthenticated","message":"No session."}'),
		)
		await api.auth.me().catch(() => undefined)
		fetchMock.mockResolvedValueOnce(reply(200, '{"id":"01J000000000000000000USR"}'))
		await api.auth.me()
		fetchMock.mockResolvedValueOnce(
			reply(401, '{"error":"unauthenticated","message":"No session."}'),
		)
		await api.auth.me().catch(() => undefined)

		expect(toLogin).toHaveBeenCalledTimes(2)
	})

	it('does not let wrong credentials pass as an expired session', async () => {
		const toLogin = vi.fn()
		setUnauthenticatedHandler(toLogin)
		fetchMock.mockResolvedValue(
			reply(401, '{"error":"invalid_credentials","message":"Wrong password."}'),
		)

		const failure = await api.auth
			.login({ username: 'ada', password: 'x' })
			.catch((e: unknown) => e)

		expect(toLogin).not.toHaveBeenCalled()
		expect(hasErrorCode(failure, 'invalid_credentials')).toBe(true)
	})
})

describe('Requests', () => {
	it('sends the session cookie and JSON', async () => {
		fetchMock.mockResolvedValue(reply(200, '{"power_state":"starting","target":"start"}'))

		await api.servers.power('01J000000000000000000SRV', { action: 'start' })

		const [url, init] = fetchMock.mock.calls[0]
		expect(url).toBe('/api/v1/servers/01J000000000000000000SRV/power')
		expect(init?.credentials).toBe('same-origin')
		expect(init?.body).toBe('{"action":"start"}')
	})

	it('passes an AbortSignal through', async () => {
		fetchMock.mockResolvedValue(reply(200, '{"servers":[],"users":{}}'))
		const controller = new AbortController()

		await api.servers.list({}, { signal: controller.signal })

		expect(fetchMock.mock.calls[0][1]?.signal).toBe(controller.signal)
	})

	it('returns nothing on a 204', async () => {
		fetchMock.mockResolvedValue(new Response(null, { status: 204 }))

		await expect(
			api.operations.dismiss('01J000000000000000000SRV', '01J000000000000000000OPX'),
		).resolves.toBeUndefined()
	})
})

describe('Paging', () => {
	it('repeats parameters the contract allows more than once', () => {
		const url = buildUrl('/operations', {
			state: 'active',
			server_id: ['01J1', '01J2'],
			limit: 100,
		})

		expect(url).toBe('/api/v1/operations?state=active&server_id=01J1&server_id=01J2&limit=100')
	})

	it('follows the after form until has_more drops', async () => {
		fetchMock
			.mockResolvedValueOnce(
				reply(
					200,
					JSON.stringify({
						path: '/',
						page_size: 2,
						total: 3,
						has_more: true,
						next_after: 'mods',
						items: [{ name: 'eula.txt' }, { name: 'mods' }],
					}),
				),
			)
			.mockResolvedValueOnce(
				reply(
					200,
					JSON.stringify({
						path: '/',
						page_size: 2,
						total: 3,
						has_more: false,
						next_after: null,
						items: [{ name: 'world' }],
					}),
				),
			)

		const page = await api.files.listAll('01J000000000000000000SRV', '/')

		expect(page.items).toHaveLength(3)
		expect(page.truncated).toBe(false)
		expect(fetchMock.mock.calls[1][0]).toBe(
			'/api/v1/servers/01J000000000000000000SRV/files/list?path=%2F&after=mods',
		)
	})
})
