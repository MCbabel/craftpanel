import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { isApiRequestError } from './client'
import { contentUrl, mail, previewUrl } from './mail'

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

function lastCall(): { url: string; init: RequestInit } {
	const [url, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit]
	return { url, init }
}

describe('the eight calls', () => {
	it('saves the body as JSON and nothing else', async () => {
		replies(200, '{"provider":"resend","state":"configured"}')

		await mail.save({
			from_address: 'panel@panel.example',
			from_name: 'craftpanel',
			reply_to: null,
			link_base: 'https://panel.example',
			daily_limit: 100,
			api_key: 're_new',
		})

		const { url, init } = lastCall()
		expect(url).toBe('/api/v1/admin/mail')
		expect(init.method).toBe('PUT')
		expect(init.credentials).toBe('same-origin')
		expect(JSON.parse(String(init.body)).api_key).toBe('re_new')
	})

	it('hangs the filters of the outbox on as a query', async () => {
		replies(200, '{"mails":[],"total":0}')
		await mail.outbox({ limit: 25, state: 'failed' })

		expect(lastCall().url).toBe('/api/v1/admin/mail/outbox?limit=25&state=failed')
	})

	it('leaves an empty filter out instead of sending it empty', async () => {
		replies(200, '{"mails":[],"total":0}')
		await mail.outbox()

		expect(lastCall().url).toBe('/api/v1/admin/mail/outbox')
	})

	it('sends nothing when deleting the key and reads no body', async () => {
		replies(204, null)
		await expect(mail.dropKey()).resolves.toBeUndefined()

		const { url, init } = lastCall()
		expect(url).toBe('/api/v1/admin/mail/key')
		expect(init.method).toBe('DELETE')
		expect(init.body).toBeUndefined()
	})

	it('puts the id of a mail into the path', async () => {
		replies(202, null)
		await mail.retry('01J8Z0K7QN9YB6R4W2M5T3XCVD')

		expect(lastCall().url).toBe(
			'/api/v1/admin/mail/outbox/01J8Z0K7QN9YB6R4W2M5T3XCVD/retry',
		)
	})
})

describe('the refusals from 19.11', () => {
	it('keep the code and the sentence of the contract', async () => {
		replies(
			502,
			'{"error":"mail_sender_rejected","message":"The domain of x is not verified at Resend."}',
		)

		const failed = await mail.test({ to: 'owner@example.com' }).catch((error: unknown) => error)
		expect(isApiRequestError(failed)).toBe(true)
		if (!isApiRequestError(failed)) return
		expect(failed.status).toBe(502)
		expect(failed.code).toBe('mail_sender_rejected')
		expect(failed.message).toContain('not verified')
	})

	it('become an error with a status even without an envelope', async () => {
		replies(404, 'nope', 'text/plain')

		const failed = await mail.settings().catch((error: unknown) => error)
		expect(isApiRequestError(failed)).toBe(true)
		if (!isApiRequestError(failed)) return
		expect(failed.status).toBe(404)
		expect(failed.code).toBe('http_404')
	})

	it('are called unauthenticated when the session is gone', async () => {
		replies(401, '')

		const failed = await mail.settings().catch((error: unknown) => error)
		expect(isApiRequestError(failed) && failed.code).toBe('unauthenticated')
	})

	it('say so when the panel itself cannot be reached', async () => {
		fetchMock.mockImplementation(() => Promise.reject(new TypeError('offline')))

		const failed = await mail.settings().catch((error: unknown) => error)
		expect(isApiRequestError(failed) && failed.code).toBe('network_unreachable')
	})
})

describe('the two paths that deliver HTML', () => {
	it('are addresses for a new tab and no fetch', () => {
		expect(previewUrl('verify_email')).toBe('/api/v1/admin/mail/preview/verify_email')
		expect(contentUrl('01J8Z0K7QN9YB6R4W2M5T3XCVD')).toBe(
			'/api/v1/admin/mail/outbox/01J8Z0K7QN9YB6R4W2M5T3XCVD/content',
		)
		expect(fetchMock).not.toHaveBeenCalled()
	})
})
