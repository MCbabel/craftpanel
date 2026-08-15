import { defineMessages } from '@modrinth/ui'

import type { PlayitAccountStatus, PlayitAgentState, PlayitBinaryState } from '@/api/playit'

export const AGENT_COLORS: Record<PlayitAgentState, string> = {
	absent: 'gray',
	starting: 'orange',
	running: 'green',
	failed: 'red',
}

export const AGENT_LABELS = defineMessages({
	absent: { id: 'admin.playit.agent.absent', defaultMessage: 'not running' },
	starting: { id: 'admin.playit.agent.starting', defaultMessage: 'starting' },
	running: { id: 'admin.playit.agent.running', defaultMessage: 'running' },
	failed: { id: 'admin.playit.agent.failed', defaultMessage: 'failed' },
}) satisfies Record<PlayitAgentState, unknown>

export const BINARY_LABELS = defineMessages({
	absent: { id: 'admin.playit.binary.absent', defaultMessage: 'not downloaded' },
	fetching: { id: 'admin.playit.binary.fetching', defaultMessage: 'downloading' },
	ready: { id: 'admin.playit.binary.ready', defaultMessage: 'ready' },
	failed: { id: 'admin.playit.binary.failed', defaultMessage: 'failed' },
}) satisfies Record<PlayitBinaryState, unknown>

export const ACCOUNT_COLORS: Record<PlayitAccountStatus, string> = {
	guest: 'orange',
	email_not_verified: 'orange',
	verified: 'green',
}

export const ACCOUNT_LABELS = defineMessages({
	guest: { id: 'admin.playit.account.guest', defaultMessage: 'guest' },
	email_not_verified: {
		id: 'admin.playit.account.email-not-verified',
		defaultMessage: 'e-mail not verified',
	},
	verified: { id: 'admin.playit.account.verified', defaultMessage: 'verified' },
}) satisfies Record<PlayitAccountStatus, unknown>
