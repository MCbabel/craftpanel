import type { DriveLink, DriveStatus } from '@/api/drive'
import { dayLeft, dayShare, linkPhase, storageLeft, storageShare } from '@/api/drive'

export type DriveStage = 'unavailable' | 'unconnected' | 'linking' | 'connected'

export interface DriveStorageView {
	usedBytes: number | null
	limitBytes: number | null
	freeBytes: number | null
	share: number | null
	nearlyFull: boolean
}

export interface DriveDayView {
	sentBytes: number
	limitBytes: number
	freeBytes: number
	share: number
	closing: boolean
	spent: boolean
}

export interface DriveView {
	stage: DriveStage
	storage: DriveStorageView
	day: DriveDayView
	broken: boolean
	lastFailure: string | null
	link: DriveLink | null
}

const NOTHING: DriveStorageView = {
	usedBytes: null,
	limitBytes: null,
	freeBytes: null,
	share: null,
	nearlyFull: false,
}

const NO_DAY: DriveDayView = {
	sentBytes: 0,
	limitBytes: 0,
	freeBytes: 0,
	share: 0,
	closing: false,
	spent: false,
}

export function driveView(status: DriveStatus | null): DriveView {
	if (status === null) {
		return {
			stage: 'unavailable',
			storage: NOTHING,
			day: NO_DAY,
			broken: false,
			lastFailure: null,
			link: null,
		}
	}

	const link = status.link !== null && linkPhase(status.link) === 'waiting' ? status.link : null
	const stage: DriveStage = !status.panel_configured
		? 'unavailable'
		: status.configured
			? 'connected'
			: link !== null
				? 'linking'
				: 'unconnected'

	return {
		stage,
		storage: storageView(status),
		day: dayView(status),
		broken: status.configured && status.state !== 'connected',
		lastFailure: status.configured ? null : status.last_error,
		link,
	}
}

function storageView(status: DriveStatus): DriveStorageView {
	const share = storageShare(status)
	return {
		usedBytes: status.storage_usage_bytes,
		limitBytes: status.storage_limit_bytes,
		freeBytes: storageLeft(status),
		share,
		nearlyFull: share !== null && share >= 0.9,
	}
}

function dayView(status: DriveStatus): DriveDayView {
	const share = dayShare(status)
	return {
		sentBytes: status.uploaded_today_bytes,
		limitBytes: status.daily_upload_limit_bytes,
		freeBytes: dayLeft(status),
		share,
		closing: share >= 0.9,
		spent: dayLeft(status) === 0 && status.daily_upload_limit_bytes > 0,
	}
}

export interface LinkCountdown {
	remaining: string
	progress: number
}

export function linkCountdown(link: DriveLink, now: number): LinkCountdown {
	const from = Date.parse(link.started_at)
	const to = Date.parse(link.expires_at)
	if (!Number.isFinite(to)) return { remaining: '0:00', progress: 0 }

	const left = Math.max(0, Math.round((to - now) / 1000))
	const remaining = `${Math.floor(left / 60)}:${String(left % 60).padStart(2, '0')}`

	if (!Number.isFinite(from) || to <= from) return { remaining, progress: 0 }
	return { remaining, progress: Math.min(1, Math.max(0, (now - from) / (to - from))) }
}

export function readableCode(link: DriveLink): string {
	return link.user_code.trim()
}
