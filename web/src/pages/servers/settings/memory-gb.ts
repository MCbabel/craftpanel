const MIB_PER_GB = 1024

export const MEMORY_MIN_GB = 1

export function gigabytes(mib: number): number {
	return Math.round((mib / MIB_PER_GB) * 10) / 10
}

export function wholeGigabytes(mib: number): number {
	return Math.floor(mib / MIB_PER_GB)
}

export function mebibytes(gb: number): number {
	return Math.round(gb * MIB_PER_GB)
}
