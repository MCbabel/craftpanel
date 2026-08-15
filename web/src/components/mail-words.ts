import { defineMessages } from '@modrinth/ui'

import type { MailDeliveryState, MailKind, MailSettings } from '@/api/mail'

export const STATE_COLORS: Record<MailSettings['state'], string> = {
	not_configured: 'gray',
	configured: 'green',
	file_sink: 'orange',
}

export const STATE_LABELS = defineMessages({
	not_configured: { id: 'admin.mail.state.not-configured', defaultMessage: 'not set up' },
	configured: { id: 'admin.mail.state.configured', defaultMessage: 'key stored' },
	file_sink: { id: 'admin.mail.state.file-sink', defaultMessage: 'writing files' },
}) satisfies Record<MailSettings['state'], unknown>

export const DELIVERY_COLORS: Record<MailDeliveryState, string> = {
	queued: 'orange',
	sending: 'blue',
	sent: 'green',
	failed: 'red',
}

export const DELIVERY_LABELS = defineMessages({
	queued: { id: 'admin.mail.delivery.queued', defaultMessage: 'waiting' },
	sending: { id: 'admin.mail.delivery.sending', defaultMessage: 'sending' },
	sent: { id: 'admin.mail.delivery.sent', defaultMessage: 'accepted' },
	failed: { id: 'admin.mail.delivery.failed', defaultMessage: 'failed' },
}) satisfies Record<MailDeliveryState, unknown>

export const KIND_LABELS = defineMessages({
	verify_email: { id: 'admin.mail.kind.verify-email', defaultMessage: 'Confirm address' },
	address_already_registered: {
		id: 'admin.mail.kind.address-already-registered',
		defaultMessage: 'Address already in use',
	},
	account_awaiting_review: {
		id: 'admin.mail.kind.account-awaiting-review',
		defaultMessage: 'Sign-up waiting',
	},
	account_approved: { id: 'admin.mail.kind.account-approved', defaultMessage: 'Account let in' },
	account_rejected: { id: 'admin.mail.kind.account-rejected', defaultMessage: 'Sign-up refused' },
	reset_password: { id: 'admin.mail.kind.reset-password', defaultMessage: 'Password reset' },
	password_changed: {
		id: 'admin.mail.kind.password-changed',
		defaultMessage: 'Password was changed',
	},
	test: { id: 'admin.mail.kind.test', defaultMessage: 'Test mail' },
}) satisfies Record<MailKind, unknown>
