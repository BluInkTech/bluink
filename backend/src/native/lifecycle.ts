// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { shutdownNativeDiagnostics } from './addon.js'

/** Host lifetime only. Rust owns the resources drained by each registered stop. */
export class HostLifecycle {
	readonly #cleanups = new Set<() => Promise<void>>()
	readonly #work = new Set<Promise<unknown>>()
	readonly #finalizers = new Set<() => Promise<void>>()
	#keepalive: ReturnType<typeof setInterval> | undefined
	#stop: Promise<void> | undefined
	#stopping = false

	get stopping(): boolean {
		return this.#stopping
	}

	checkRunning(): void {
		if (this.#stopping) throw new Error('The application is shutting down.')
	}

	/** Register synchronously, before initialization or command execution. */
	register(cleanup: () => Promise<void>): () => void {
		this.checkRunning()
		this.#cleanups.add(cleanup)
		return () => this.#cleanups.delete(cleanup)
	}

	/** Terminal restoration runs after applications and diagnostics have drained. */
	registerFinalizer(finalizer: () => Promise<void>): void {
		this.checkRunning()
		this.#finalizers.add(finalizer)
	}

	/** Track only the command body, never its outer error/cleanup handler. */
	run<T>(operation: () => Promise<T>): Promise<T> {
		this.checkRunning()
		this.#keepalive ??= setInterval(() => {}, 1000)
		const work = Promise.resolve().then(() => {
			this.checkRunning()
			return operation()
		})
		this.#work.add(work)
		return work.finally(() => this.#work.delete(work))
	}

	/** Repeated callers await the same result, including any cleanup failure. */
	stop(): Promise<void> {
		if (this.#stop) return this.#stop
		this.#stopping = true
		this.#stop = (async () => {
			const failures: unknown[] = []
			const results = await Promise.allSettled(
				[...this.#cleanups].map((cleanup) => Promise.resolve().then(cleanup)),
			)
			for (const result of results) {
				if (result.status === 'rejected') failures.push(result.reason)
			}
			// Native stop drains resources; pending host stages must also settle
			// before the process-wide diagnostic writer is shut down.
			await Promise.allSettled([...this.#work])
			try {
				await shutdownNativeDiagnostics()
			} catch (error) {
				failures.push(error)
			}
			const finalized = await Promise.allSettled(
				[...this.#finalizers].map((finalize) =>
					Promise.resolve().then(finalize),
				),
			)
			for (const result of finalized) {
				if (result.status === 'rejected') failures.push(result.reason)
			}
			if (failures.length)
				throw new AggregateError(failures, 'Backend cleanup failed')
		})().finally(() => clearInterval(this.#keepalive))
		return this.#stop
	}
}

export const hostLifecycle = new HostLifecycle()
