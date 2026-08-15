import { computed, ref } from 'vue'

export const themeChoices = ['system', 'light', 'dark', 'oled', 'retro'] as const

export type ThemeChoice = (typeof themeChoices)[number]
export type Theme = Exclude<ThemeChoice, 'system'>

const STORAGE_KEY = 'craftpanel.theme'

const themes: readonly Theme[] = ['light', 'dark', 'oled', 'retro']
const darkThemes: readonly Theme[] = ['dark', 'oled', 'retro']

const choice = ref<ThemeChoice>('system')
const systemTheme = ref<Theme>('dark')
const theme = computed<Theme>(() => (choice.value === 'system' ? systemTheme.value : choice.value))

function apply(): void {
	const root = document.documentElement
	for (const mode of themes) {
		root.classList.toggle(`${mode}-mode`, mode === theme.value)
	}
	root.style.colorScheme = darkThemes.includes(theme.value) ? 'dark' : 'light'
}

function isChoice(value: string | null): value is ThemeChoice {
	return themeChoices.some((option) => option === value)
}

function remembered(): ThemeChoice | null {
	try {
		const value = localStorage.getItem(STORAGE_KEY)
		return isChoice(value) ? value : null
	} catch {
		return null
	}
}

function remember(value: ThemeChoice): void {
	try {
		localStorage.setItem(STORAGE_KEY, value)
	} catch {
	}
}

function setTheme(value: ThemeChoice): void {
	choice.value = value
	remember(value)
	apply()
}

export function startTheme(): void {
	const query = matchMedia('(prefers-color-scheme: light)')
	systemTheme.value = query.matches ? 'light' : 'dark'
	query.addEventListener('change', (event) => {
		systemTheme.value = event.matches ? 'light' : 'dark'
		apply()
	})
	choice.value = remembered() ?? 'system'
	apply()
}

export function useTheme() {
	return { theme, choice, systemTheme, setTheme }
}
