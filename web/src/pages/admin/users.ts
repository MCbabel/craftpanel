import type { MailSettings } from '@/api/mail'

export type ResetLinkProblem = 'no-address' | 'no-mail' | 'no-link-base' | 'unsaved-address'

export type MailReadiness = Pick<MailSettings, 'state' | 'link_base'> | null

export function resetLinkProblem(
	account: { email: string | null },
	mail: MailReadiness,
	typed?: string,
): ResetLinkProblem | null {
	if (typed !== undefined && !sameAddress(typed, account.email)) return 'unsaved-address'
	if (account.email === null) return 'no-address'
	if (mail === null) return null
	if (mail.state === 'not_configured') return 'no-mail'
	return mail.link_base === null ? 'no-link-base' : null
}

export function addressFromField(typed: string): string | null {
	return typed.trim() === '' ? null : typed.trim()
}

export function sameAddress(typed: string, stored: string | null): boolean {
	const wanted = addressFromField(typed)?.toLowerCase() ?? null
	return wanted === (stored?.trim().toLowerCase() ?? null)
}
