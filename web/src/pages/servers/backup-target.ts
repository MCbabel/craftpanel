import type { BackupTarget } from '@/api/drive'
import type { BackupLocation, DriveFileState } from '@/api/types'

export interface DriveFacts {
	location: BackupLocation
	state: DriveFileState | null
	verified: boolean | null
	contentChanged: boolean
	webLink: string | null
}

const LOCAL: DriveFacts = {
	location: 'local',
	state: null,
	verified: null,
	contentChanged: false,
	webLink: null,
}

const STATES: readonly DriveFileState[] = ['present', 'missing', 'trashed', 'unreachable']

export function driveFactsOf(backup: unknown): DriveFacts {
	if (typeof backup !== 'object' || backup === null) return LOCAL
	const row = backup as {
		location?: unknown
		drive_state?: unknown
		drive_verified?: unknown
		drive_content_changed?: unknown
		drive_web_link?: unknown
	}
	if (row.location !== 'drive') return LOCAL

	return {
		location: 'drive',
		state: STATES.includes(row.drive_state as DriveFileState)
			? (row.drive_state as DriveFileState)
			: null,
		verified: typeof row.drive_verified === 'boolean' ? row.drive_verified : null,
		contentChanged: row.drive_content_changed === true,
		webLink: typeof row.drive_web_link === 'string' ? row.drive_web_link : null,
	}
}

export function notRestorable(facts: DriveFacts): boolean {
	return facts.state === 'missing' || facts.state === 'trashed'
}

export function unconfirmed(facts: DriveFacts): boolean {
	return facts.state === 'present' && facts.verified === false && !facts.contentChanged
}

export function noLongerOurs(facts: DriveFacts): boolean {
	return facts.state === 'present' && facts.contentChanged
}

export function openableInDrive(facts: DriveFacts): boolean {
	return facts.location === 'drive' && facts.webLink !== null
}

export function backupImpossible(target: BackupTarget | null): boolean {
	if (target === null || target.effective_target !== 'drive') return false
	return target.reason === 'not_connected' || target.reason === 'not_configured'
}

export function needsOwnersDrive(target: BackupTarget | null): boolean {
	return backupImpossible(target) && target?.reason === 'not_connected'
}

export function targetIsChoosable(target: BackupTarget | null): boolean {
	if (target === null) return false
	if (target.policy !== 'user_choice') return false
	return target.reason === 'ok' || target.target === 'drive'
}
