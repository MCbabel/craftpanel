import type { MailSettings, UpdateMailSettingsRequest } from '@/api/mail'

export interface MailDraft {
	from_address: string
	from_name: string
	reply_to: string
	link_base: string
	daily_limit: number
}

export function draftOf(settings: MailSettings): MailDraft {
	return {
		from_address: settings.from_address,
		from_name: settings.from_name,
		reply_to: settings.reply_to ?? '',
		link_base: settings.link_base ?? '',
		daily_limit: settings.daily_limit,
	}
}

export function draftChanged(draft: MailDraft, settings: MailSettings): boolean {
	const before = draftOf(settings)
	return (
		draft.from_address.trim() !== before.from_address ||
		draft.from_name.trim() !== before.from_name ||
		draft.reply_to.trim() !== before.reply_to ||
		draft.link_base.trim() !== before.link_base ||
		draft.daily_limit !== before.daily_limit
	)
}

export function draftRequest(draft: MailDraft, apiKey?: string): UpdateMailSettingsRequest {
	const body: UpdateMailSettingsRequest = {
		from_address: draft.from_address.trim(),
		from_name: draft.from_name.trim(),
		reply_to: draft.reply_to.trim() === '' ? null : draft.reply_to.trim(),
		link_base: draft.link_base.trim() === '' ? null : draft.link_base.trim(),
		daily_limit: draft.daily_limit,
	}
	if (apiKey !== undefined) body.api_key = apiKey
	return body
}

export function addressLooksRight(address: string): boolean {
	const text = address.trim()
	const parts = text.split('@')
	if (parts.length !== 2) return false
	const [local, domain] = parts as [string, string]
	return (
		local.length > 0 &&
		domain.includes('.') &&
		!domain.startsWith('.') &&
		!domain.endsWith('.') &&
		text.length <= 254 &&
		!/\s/.test(text)
	)
}

export type LinkBaseProblem = 'missing' | 'no-scheme' | 'insecure' | null

export function linkBaseProblem(base: string): LinkBaseProblem {
	const text = base.trim()
	if (text === '') return 'missing'
	if (text.startsWith('https://')) return null
	if (text.startsWith('http://')) return 'insecure'
	return 'no-scheme'
}

export function exampleLink(base: string): string | null {
	const text = base.trim().replace(/\/+$/, '')
	if (text === '' || linkBaseProblem(text) === 'no-scheme') return null
	return `${text}/verify-email#…`
}

export function senderPreview(name: string, address: string): string {
	const clean = name.trim().replace(/[<>",]/g, '').trim()
	const to = address.trim()
	return clean === '' ? to : `${clean} <${to}>`
}

export const RESEND_TEST_SENDER = 'onboarding@resend.dev'

export function sendingToStrangersWorks(settings: MailSettings): boolean {
	return settings.state === 'file_sink' || settings.from_address.trim() !== RESEND_TEST_SENDER
}
