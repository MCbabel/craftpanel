import type { JavaMajorEntry, JavaRuntimeOverview } from '@/api'

export const POLL_MS = 700

export type RuntimeStanding = 'laid' | 'system' | 'absent'

export function busy(entry: JavaMajorEntry): boolean {
	return entry.job?.running ?? false
}

export function pollDelay(overview: JavaRuntimeOverview | null): number | null {
	return (overview?.majors ?? []).some(busy) ? POLL_MS : null
}

export function standingOf(entry: JavaMajorEntry): RuntimeStanding {
	if (entry.runtime !== null) return 'laid'
	if (entry.system !== null) return 'system'
	return 'absent'
}

export function canFetch(overview: JavaRuntimeOverview, entry: JavaMajorEntry): boolean {
	return (
		overview.architecture !== null &&
		entry.fetchable &&
		!busy(entry) &&
		entry.running.length === 0
	)
}

export function canRemove(entry: JavaMajorEntry): boolean {
	return entry.runtime !== null && !busy(entry) && entry.running.length === 0
}

export function failureOf(entry: JavaMajorEntry): string | null {
	const job = entry.job
	return job !== null && job !== undefined && !job.running ? job.failure : null
}
