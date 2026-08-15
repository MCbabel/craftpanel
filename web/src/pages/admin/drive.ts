import type { BackupTargetPolicy, DriveAdminOverview } from '@/api/drive'

export interface DriveDraft {
	client_id: string
	client_secret: string
	target_policy: BackupTargetPolicy
	folder_name: string
}

export const POLICIES: readonly BackupTargetPolicy[] = ['user_choice', 'drive_only', 'local_only']

export function blankDraft(): DriveDraft {
	return { client_id: '', client_secret: '', target_policy: 'user_choice', folder_name: 'craftpanel-backups' }
}

export function draftOf(overview: DriveAdminOverview): DriveDraft {
	return {
		client_id: overview.client_id ?? '',
		client_secret: '',
		target_policy: overview.target_policy,
		folder_name: overview.folder_name,
	}
}

export function secretToSend(draft: DriveDraft): string | undefined {
	return draft.client_secret === '' ? undefined : draft.client_secret
}
