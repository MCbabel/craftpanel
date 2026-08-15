import { describe, expect, it } from 'vitest'

import type { MailSettings } from '@/api/mail'
import {
	addressLooksRight,
	draftChanged,
	draftOf,
	draftRequest,
	exampleLink,
	linkBaseProblem,
	RESEND_TEST_SENDER,
	senderPreview,
	sendingToStrangersWorks,
} from '@/pages/admin/mail'

function settings(over: Partial<MailSettings> = {}): MailSettings {
	return {
		provider: 'resend',
		state: 'not_configured',
		key_set_at: null,
		from_address: RESEND_TEST_SENDER,
		from_name: 'craftpanel',
		reply_to: null,
		link_base: null,
		example_link: null,
		sink_path: null,
		daily_limit: 100,
		sent_today: 0,
		queued: 0,
		failed: 0,
		last_test_at: null,
		last_error: null,
		last_error_at: null,
		...over,
	}
}

describe('the draft of the form', () => {
	it('turns null into an empty text and back again', () => {
		const draft = draftOf(settings())
		expect(draft.reply_to).toBe('')
		expect(draft.link_base).toBe('')

		const body = draftRequest(draft)
		expect(body.reply_to).toBeNull()
		expect(body.link_base).toBeNull()
	})

	it('leaves the key alone as long as none was typed in', () => {
		const body = draftRequest(draftOf(settings()))
		expect('api_key' in body).toBe(false)

		expect(draftRequest(draftOf(settings()), 're_new').api_key).toBe('re_new')
		expect(draftRequest(draftOf(settings()), '').api_key).toBe('')
	})

	it('notices a change and ignores mere whitespace', () => {
		const before = settings({ link_base: 'https://panel.example' })
		const draft = draftOf(before)
		expect(draftChanged(draft, before)).toBe(false)

		expect(draftChanged({ ...draft, link_base: '  https://panel.example  ' }, before)).toBe(false)
		expect(draftChanged({ ...draft, link_base: 'https://other.example' }, before)).toBe(true)
		expect(draftChanged({ ...draft, daily_limit: 50 }, before)).toBe(true)
	})
})

describe('the thin address check', () => {
	it('accepts what Resend can accept', () => {
		expect(addressLooksRight('anna@example.com')).toBe(true)
		expect(addressLooksRight(' anna+panel@sub.example.co.uk ')).toBe(true)
		expect(addressLooksRight(RESEND_TEST_SENDER)).toBe(true)
	})

	it('rejects what is no address at all', () => {
		expect(addressLooksRight('')).toBe(false)
		expect(addressLooksRight('anna')).toBe(false)
		expect(addressLooksRight('@example.com')).toBe(false)
		expect(addressLooksRight('anna@example')).toBe(false)
		expect(addressLooksRight('anna@.com')).toBe(false)
		expect(addressLooksRight('anna@example.com anna@evil.com')).toBe(false)
		expect(addressLooksRight('two@at@example.com')).toBe(false)
	})
})

describe('the panel address', () => {
	it('tells apart missing, without a scheme, and unencrypted', () => {
		expect(linkBaseProblem('')).toBe('missing')
		expect(linkBaseProblem('   ')).toBe('missing')
		expect(linkBaseProblem('panel.example')).toBe('no-scheme')
		expect(linkBaseProblem('http://192.168.1.10:8080')).toBe('insecure')
		expect(linkBaseProblem('https://panel.example')).toBeNull()
	})

	it('shows the link that comes out of it', () => {
		expect(exampleLink('https://panel.example')).toBe('https://panel.example/verify-email#…')
		expect(exampleLink('https://panel.example/')).toBe('https://panel.example/verify-email#…')
		expect(exampleLink('')).toBeNull()
		expect(exampleLink('panel.example')).toBeNull()
	})
})

describe('the sender', () => {
	it('shows the header the way it stands in the mailbox', () => {
		expect(senderPreview('craftpanel', RESEND_TEST_SENDER)).toBe(
			`craftpanel <${RESEND_TEST_SENDER}>`,
		)
		expect(senderPreview('', 'panel@panel.example')).toBe('panel@panel.example')
		expect(senderPreview('The "panel", <ha>', 'panel@panel.example')).toBe(
			'The panel ha <panel@panel.example>',
		)
	})

	it('says that the preset only goes to one\'s own account address', () => {
		expect(sendingToStrangersWorks(settings())).toBe(false)
		expect(sendingToStrangersWorks(settings({ from_address: 'panel@panel.example' }))).toBe(true)
		expect(sendingToStrangersWorks(settings({ state: 'file_sink' }))).toBe(true)
	})
})
