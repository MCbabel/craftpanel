import { ref } from 'vue'

export const localeChoices = ['en-US', 'de-DE'] as const

export type LocaleChoice = (typeof localeChoices)[number]

const STORAGE_KEY = 'craftpanel.locale'

const defaultLocale: LocaleChoice = 'en-US'

const choice = ref<LocaleChoice>(defaultLocale)

export function isLocale(value: string | null | undefined): value is LocaleChoice {
	return localeChoices.some((option) => option === value)
}

function apply(): void {
	document.documentElement.lang = choice.value
}

function remembered(): LocaleChoice | null {
	try {
		const value = localStorage.getItem(STORAGE_KEY)
		return isLocale(value) ? value : null
	} catch {
		return null
	}
}

function remember(value: LocaleChoice): void {
	try {
		localStorage.setItem(STORAGE_KEY, value)
	} catch {
	}
}

export function browserLocale(): LocaleChoice {
	const wishes = navigator.languages?.length ? navigator.languages : [navigator.language]
	for (const wish of wishes) {
		if (isLocale(wish)) {
			return wish
		}
		const language = String(wish ?? '')
			.split('-')[0]
			.toLowerCase()
		const relative = localeChoices.find((option) => option.split('-')[0] === language)
		if (relative) {
			return relative
		}
	}
	return defaultLocale
}

export function setLocale(value: string): void {
	if (!isLocale(value)) {
		return
	}
	choice.value = value
	remember(value)
	apply()
}

export function startLocale(): void {
	choice.value = remembered() ?? browserLocale()
	apply()
}

export function useLocale() {
	return { locale: choice, setLocale }
}
