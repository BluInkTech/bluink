// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { readFile, writeFile } from 'node:fs/promises'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { createApplication } from './addon.js'
import { seedPages, withTempDir } from '../test-utils/fixtures.js'

const execute = promisify(execFile)
const cli = fileURLToPath(new URL('../../bluink', import.meta.url))

async function command(cwd: string, ...args: string[]) {
	try {
		const result = await execute(process.execPath, [cli, ...args], {
			cwd,
			timeout: 30_000,
			maxBuffer: 4 * 1024 * 1024,
			env: { ...process.env, BLUINK_LOG_LEVEL: 'error' },
		})
		return { ...result, code: 0 }
	} catch (error) {
		const failure = error as Error & {
			code: number | string
			stdout: string
			stderr: string
		}
		if (typeof failure.code !== 'number') throw error
		return {
			code: failure.code,
			stdout: failure.stdout,
			stderr: failure.stderr,
		}
	}
}

async function fixture(root: string) {
	const pages = await seedPages(
		createApplication({
			projectRoot: root,
			timezone: 'UTC',
			cacheLocation: 'project',
		}),
		['Home 文学', 'Chapter'].map((title) => ({
			title,
			text: `${title} body <safe> & complete.`,
		})),
	)
	return { home: pages[0]!, chapter: pages[1]! }
}

test('compiled Export CLI forwards flags, resolves packaged PDF resources and separates output from errors', {
	timeout: 60_000,
}, async () => {
	await withTempDir(async (directory) => {
		const root = path.join(directory, 'project')
		const { home } = await fixture(root)
		const checked = await command(
			directory,
			'export',
			'html',
			'--project',
			root,
			'--note',
			home.id,
			'--check',
		)
		assert.equal(checked.code, 0, checked.stderr)
		assert.equal(JSON.parse(checked.stdout).pageId, home.id)
		assert.equal(checked.stderr, '')

		const generated = await command(
			directory,
			'export',
			'pdf',
			'--project',
			root,
			'--note',
			home.id,
			'--out',
			'./note.pdf',
		)
		assert.equal(generated.code, 0, generated.stderr)
		const output = path.join(directory, 'note.pdf')
		assert.equal(generated.stdout.trim(), output)
		assert.equal((await readFile(output)).subarray(0, 5).toString(), '%PDF-')

		const invalid = await command(
			directory,
			'export',
			'html',
			'--unknown-option',
		)
		assert.equal(invalid.code, 1)
		assert.equal(invalid.stdout, '')
		assert.match(invalid.stderr, /Unknown option/)
	})
})

test('compiled Publication CLI forwards create, inspect, plan and generate actions with JSON results', {
	timeout: 60_000,
}, async () => {
	await withTempDir(async (directory) => {
		const root = path.join(directory, 'project')
		const { home, chapter } = await fixture(root)
		await writeFile(
			path.join(directory, 'book.json'),
			JSON.stringify({
				title: 'CLI book',
				content: {
					kind: 'publication',
					preset: 'book',
					selectedOutput: 'site',
					rootPageId: home.id,
					outline: [{ kind: 'page', pageId: chapter.id, children: [] }],
					selectionRules: [],
					themeId: 'classic',
					targets: {
						pdf: { profile: 'default', outsideLinks: 'error' },
						html: { outsideLinks: 'error' },
						docx: { outsideLinks: 'error' },
						epub: { outsideLinks: 'error' },
						site: { layout: 'documentation', outsideLinks: 'error' },
					},
				},
			}),
		)
		const created = await command(
			directory,
			'publication',
			'create',
			'--project',
			root,
			'--input',
			'./book.json',
		)
		assert.equal(created.code, 0, created.stderr)
		const id = created.stdout.trim()
		assert.match(id, /^[0-9a-f-]{36}$/)

		const inspected = await command(
			directory,
			'publication',
			'inspect',
			'--project',
			root,
			'--id',
			id,
		)
		assert.equal(inspected.code, 0, inspected.stderr)
		assert.equal(JSON.parse(inspected.stdout).meta.title, 'CLI book')

		const planned = await command(
			directory,
			'publication',
			'plan',
			'--project',
			root,
			'--id',
			id,
			'--format',
			'site',
		)
		assert.equal(planned.code, 0, planned.stderr)
		assert.equal(JSON.parse(planned.stdout).pages.length, 2)

		const generated = await command(
			directory,
			'publication',
			'generate',
			'--project',
			root,
			'--id',
			id,
			'--format',
			'site',
			'--output',
			'./site',
		)
		assert.equal(generated.code, 0, generated.stderr)
		assert.equal(
			JSON.parse(generated.stdout).output,
			path.join(directory, 'site'),
		)
		assert.ok(
			(await readFile(path.join(directory, 'site/index.html'))).length > 0,
		)
	})
})
