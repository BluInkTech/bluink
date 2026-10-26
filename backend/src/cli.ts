// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import path from 'node:path'
import type { ParseArgsOptionDescriptor } from 'node:util'
import { parseArgs } from 'node:util'
import pkg from '../package.json' with { type: 'json' }
import logger, { isServerConsoleActive } from './logger.js'
import { explainPortInUse } from './port-in-use.js'
import { hostLifecycle } from './native/lifecycle.js'
import { createAppState } from './state.js'

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/**
 * Extends the built-in ParseArgsOptionDescriptor with a human-readable help
 * string used when printing usage output.
 */
type OptionDescriptor = ParseArgsOptionDescriptor & { help: string }
type OptionsSpec = Record<string, OptionDescriptor>

type OptionValues = Record<
	string,
	string | boolean | (string | boolean)[] | undefined
>

interface CommandDefinition {
	description: string
	options: OptionsSpec
	positionals?: boolean
	positionalName?: string
	handler: (values: OptionValues, positionals: string[]) => Promise<void> | void
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Strip the extra `help` field so the object is safe to pass to `parseArgs`. */
function toParseArgsOptions(
	spec: OptionsSpec,
): Record<string, ParseArgsOptionDescriptor> {
	const result: Record<string, ParseArgsOptionDescriptor> = {}
	for (const [key, { help: _help, ...descriptor }] of Object.entries(spec)) {
		result[key] = descriptor
	}
	return result
}

/** Render a table of options from an OptionsSpec. */
function printOptions(spec: OptionsSpec): void {
	for (const [name, desc] of Object.entries(spec)) {
		const short = desc.short ? `-${desc.short}, ` : '    '
		logger.info(`  ${short}--${name.padEnd(14)}${desc.help}`)
	}
}

// ---------------------------------------------------------------------------
// Global options (available on every invocation)
// ---------------------------------------------------------------------------

const globalOptions = {
	help: { type: 'boolean', short: 'h', default: false, help: 'Show help' },
	version: {
		type: 'boolean',
		short: 'v',
		default: false,
		help: `Show version (${pkg.version})`,
	},
} satisfies OptionsSpec

// ---------------------------------------------------------------------------
// Help printers
// ---------------------------------------------------------------------------

function printOverallHelp(): void {
	logger.info(`Usage: ${pkg.name} [options] <command> [command-options]`)
	logger.newLine()
	logger.info('Global options:')
	printOptions(globalOptions)
	logger.newLine()
	logger.info('Commands:')
	for (const [name, def] of commands) {
		logger.info(`  ${name.padEnd(18)}${def.description}`)
	}
	logger.newLine()
	logger.info(
		`Run '${pkg.name} <command> --help' for command-specific options.`,
	)
}

function printCommandHelp(name: string, def: CommandDefinition): void {
	logger.info(
		`Usage: ${pkg.name} ${name}${def.positionals ? ` <${def.positionalName ?? 'format'}>` : ''} [options]`,
	)
	logger.newLine()
	logger.info(def.description)
	const entries = Object.entries(def.options)
	if (entries.length > 0) {
		logger.newLine()
		logger.info('Options:')
		printOptions(def.options)
	}
	logger.newLine()
	logger.info('Global options:')
	printOptions(globalOptions)
}

// ---------------------------------------------------------------------------
// Application lifecycle
// ---------------------------------------------------------------------------

let signalStop: Promise<void> | undefined
const _handleSignal = () => {
	if (signalStop) return
	if (isServerConsoleActive()) {
		logger.diagnostic({
			timestampMs: Date.now(),
			level: 'info',
			target: 'bluink.host',
			name: 'shutdown_requested',
		})
	} else {
		process.stderr.write('Performing cleanup\n')
	}
	signalStop = hostLifecycle.stop().catch(() => {
		process.stderr.write('Backend cleanup failed\n')
		process.exitCode = 1
	})
}

process.on('SIGINT', _handleSignal)
process.on('SIGTERM', _handleSignal)
process.on('SIGBREAK', _handleSignal) // Is delivered on Windows when Ctrl+Break is pressed.
process.on('SIGHUP', _handleSignal)

process.on('uncaughtException', async (e: Error) => {
	if (isServerConsoleActive()) logger.error(e.message)
	process.exitCode = 1
	try {
		await hostLifecycle.stop()
	} catch {
		process.stderr.write('Backend cleanup failed\n')
	}
	logger.error('Application stopped due to an unhandled error')
	logger.error(e.message)
})

// ---------------------------------------------------------------------------
// Commands registry
// ---------------------------------------------------------------------------

const commands = new Map<string, CommandDefinition>()

commands.set('server', {
	description: 'Start the BluInk HTTP server',
	options: {
		'no-tui': {
			type: 'boolean',
			default: false,
			help: 'Stream plain server output without interactive controls',
		},
		project: {
			type: 'string',
			short: 'P',
			help: 'Bluink project folder (default: current directory)',
		},
		port: {
			type: 'string',
			short: 'p',
			default: '9000',
			help: 'Port number to listen on (default: 9000)',
		},
	} satisfies OptionsSpec,
	handler: async (values) => {
		const port = Number(values['port'] ?? 9000)
		const projectPath = values['project']
		if (typeof projectPath === 'string') {
			process.env['BLUINK_PROJECT_DIR'] = path.resolve(projectPath)
		}
		const state = await createAppState(values['no-tui'] ? 'plain' : 'auto')
		hostLifecycle.checkRunning()
		try {
			await state.application.startHttp(port)
		} catch (error) {
			throw explainPortInUse(error, port)
		}
	},
})

commands.set('export', {
	description: 'Export one note as a PDF, Word, EPUB or HTML file',
	positionals: true,
	options: {
		project: {
			type: 'string',
			short: 'P',
			help: 'Existing Bluink project folder',
		},
		note: { type: 'string', help: 'Page ID to export as an article' },
		'include-linked': {
			type: 'boolean',
			help: 'Follow internal page links; HTML produces a website',
		},
		'outside-links': {
			type: 'string',
			help: 'Links outside the export: error or text-only',
		},
		profile: {
			type: 'string',
			help: 'PDF profile: default or accessible (PDF/UA-1)',
		},
		out: { type: 'string', short: 'o', help: 'Output file path' },
		check: { type: 'boolean', help: 'Validate without generating a file' },
		theme: {
			type: 'string',
			help: 'Publication theme for this note',
		},
		'trust-theme-sha256': {
			type: 'string',
			help: 'Exact reviewed project-theme package hash',
		},
	} satisfies OptionsSpec,
	handler: async (values, positionals) => {
		const { runApplicationCommand } = await import('./native/command.js')
		await runApplicationCommand('export', values, positionals)
	},
})

commands.set('publication', {
	description: 'Create, inspect, plan, or generate a Publication',
	positionals: true,
	positionalName: 'create|inspect|plan|generate',
	options: {
		project: {
			type: 'string',
			short: 'P',
			help: 'Existing Bluink project folder',
		},
		id: {
			type: 'string',
			help: 'Publication item ID for inspect, plan, or generate',
		},
		input: { type: 'string', help: 'JSON file for create' },
		format: {
			type: 'string',
			help: 'pdf, docx, epub, html, or site, required for plan or generate',
		},
		output: { type: 'string', help: 'New PDF/Word/HTML file or site folder' },
		'trust-theme-sha256': {
			type: 'string',
			help: 'Exact reviewed project-theme package hash',
		},
	} satisfies OptionsSpec,
	handler: async (values, positionals) => {
		const { runApplicationCommand } = await import('./native/command.js')
		await runApplicationCommand('publication', values, positionals)
	},
})

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

async function main(): Promise<void> {
	// Phase 1 — parse only the args that appear BEFORE the command name so that
	// `--help` / `-h` after a sub-command doesn't get mistaken for the global
	// help flag and accidentally show the top-level usage instead of the
	// command-specific one.
	const rawArgs = process.argv.slice(2)
	const commandIdx = rawArgs.findIndex((a) => !a.startsWith('-'))
	const commandName =
		commandIdx === -1
			? rawArgs.length === 0
				? 'server'
				: undefined
			: rawArgs[commandIdx]
	const preCommandArgs =
		commandIdx === -1 ? rawArgs : rawArgs.slice(0, commandIdx)

	const { values: globalValues } = parseArgs({
		args: preCommandArgs,
		options: toParseArgsOptions(globalOptions),
		allowPositionals: false,
		strict: false,
	})

	if (globalValues['version']) {
		logger.info(`${pkg.name} v${pkg.version}`)
		return
	}

	if (!commandName || globalValues['help']) {
		printOverallHelp()
		if (!commandName) process.exitCode = 1
		return
	}

	const def = commands.get(commandName)
	if (!def) {
		logger.error(`Unknown command: '${commandName}'`)
		logger.newLine()
		printOverallHelp()
		process.exitCode = 1
		return
	}

	// Phase 2 — strict parse of args that follow the command positional.
	const cmdArgs = commandIdx === -1 ? [] : rawArgs.slice(commandIdx + 1)

	const mergedOptions = toParseArgsOptions({ ...def.options, ...globalOptions })
	const { values: cmdValues, positionals } = parseArgs({
		args: cmdArgs,
		options: mergedOptions,
		allowPositionals: def.positionals ?? false,
		strict: true,
	})

	if (cmdValues['help']) {
		printCommandHelp(commandName, def)
		return
	}

	await hostLifecycle.run(async () => {
		await def.handler(cmdValues, positionals)
	})
	if (commandName !== 'server') await hostLifecycle.stop()
}

main().catch(async (e: Error) => {
	// Native initialization/commands reject with this error when a registered
	// signal stops them. Other errors still produce a failed exit.
	const interrupted =
		hostLifecycle.stopping && e.message === 'The application is shutting down.'
	if (!interrupted) {
		if (isServerConsoleActive()) logger.error(e.message)
		process.exitCode = 1
	}
	try {
		await hostLifecycle.stop()
	} catch {
		process.stderr.write('Backend cleanup failed\n')
		process.exitCode = 1
	}
	if (!interrupted) logger.error(e.message)
})
