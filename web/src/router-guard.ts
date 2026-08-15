import { bounceWhenSignedIn, publicRouteNames } from '@/pages/auth/routes'

export interface RouteWish {
	name: string
	adminOnly: boolean
}

export interface Visitor {
	isAdmin: boolean
	mustChangePassword: boolean
}

export type RouteDecision =
	| 'allow'
	| 'to-login'
	| 'to-change-password'
	| 'to-servers'
	| 'bounce-signed-in'

export function decideRoute(wish: RouteWish, visitor: Visitor | null): RouteDecision {
	if (visitor === null) return publicRouteNames.has(wish.name) ? 'allow' : 'to-login'

	if (bounceWhenSignedIn.has(wish.name)) return 'bounce-signed-in'
	if (publicRouteNames.has(wish.name)) return 'allow'

	if (visitor.mustChangePassword && wish.name !== 'change-password') return 'to-change-password'
	if (wish.adminOnly && !visitor.isAdmin) return 'to-servers'
	return 'allow'
}
