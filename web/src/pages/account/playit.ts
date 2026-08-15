import type { PlayitClaim, PlayitStatus } from '@/api/playit'

export type PlayitStage = 'unconnected' | 'quiet' | 'live'

export interface PlayitPortsView {
	used: number
	limit: number
	free: number
	forOthers: number
	full: boolean
}

export interface PlayitView {
	stage: PlayitStage
	ports: PlayitPortsView
	notSelfManaged: boolean
	guest: boolean
}

const NOTHING: PlayitPortsView = { used: 0, limit: 0, free: 0, forOthers: 0, full: false }

export function playitView(status: PlayitStatus | null): PlayitView {
	if (status === null) {
		return { stage: 'unconnected', ports: NOTHING, notSelfManaged: false, guest: false }
	}

	const ports = portsView(status)
	return {
		stage: !status.configured ? 'unconnected' : ports.used === 0 ? 'quiet' : 'live',
		ports,
		notSelfManaged:
			status.configured && status.checked_at !== null && !status.is_self_managed,
		guest: status.account_status === 'guest',
	}
}

function portsView(status: PlayitStatus): PlayitPortsView {
	const { used, limit, for_others: forOthers } = status.ports
	return {
		used,
		limit,
		free: Math.max(0, limit - used),
		forOthers,
		full: used >= limit,
	}
}

export interface ClaimCountdown {
	remaining: string
	progress: number
}

export function claimCountdown(claim: PlayitClaim, now: number): ClaimCountdown {
	const from = Date.parse(claim.started_at)
	const to = Date.parse(claim.expires_at)
	if (!Number.isFinite(to)) return { remaining: '0:00', progress: 0 }

	const left = Math.max(0, Math.round((to - now) / 1000))
	const remaining = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`

	if (!Number.isFinite(from) || to <= from) return { remaining, progress: 0 }
	return { remaining, progress: Math.min(1, Math.max(0, (now - from) / (to - from))) }
}
