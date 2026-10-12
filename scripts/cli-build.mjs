// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

import { cp, mkdir, readFile, rm } from 'node:fs/promises'
import path from 'node:path'
import { auditDirectory } from './package-audit.mjs'
import { publicIdentity } from './release-identity.mjs'
import {
	fileHash,
	json,
	npm,
	root,
	run,
	sha256,
	writeJson,
} from './release-utils.mjs'
import { rustNotices } from './rust-notices.mjs'

export async function contractHash() {
	return sha256(
		Buffer.concat(
			await Promise.all(
				['types.ts', 'runtime.ts', 'wire.ts'].map(async (name) =>
					Buffer.concat([
						Buffer.from(`${name}\0`),
						await readFile(
							path.join(root, 'backend/contracts/generated', name),
						),
					]),
				),
			),
		),
	)
}

const buildFileAllowed = (file) =>
	/^(?:build\.json|THIRD_PARTY_NOTICES\.txt|dist\/(?:src\/)?(?:native\/)?[\w-]+\.js)$/.test(
		file,
	)

/** Build once on a verified native runner; assembly consumes only these bytes. */
export async function prepareCliBuild(directory, identity) {
	await rm(path.join(root, 'backend/dist'), { recursive: true, force: true })
	await npm(['--workspace', 'backend', 'run', 'build'])
	await rm(directory, { recursive: true, force: true })
	await mkdir(directory, { recursive: true })
	await cp(path.join(root, 'backend/dist'), path.join(directory, 'dist'), {
		recursive: true,
	})
	// TypeScript copies the imported source manifest; assembly writes the public one.
	await rm(path.join(directory, 'dist/package.json'), { force: true })
	await rustNotices(path.join(directory, 'THIRD_PARTY_NOTICES.txt'))
	const files = await auditDirectory(directory, {
		allowed: buildFileAllowed,
		required: ['dist/index.js', 'THIRD_PARTY_NOTICES.txt'],
	})
	await writeJson(path.join(directory, 'build.json'), {
		format: 'bluink.cli-build',
		formatVersion: 1,
		version: identity.version,
		publicCommit: identity.publicCommit,
		contractHash: await contractHash(),
		toolchain: {
			node: process.version,
			npm: (await npm(['--version'], { capture: true })).trim(),
			rust: (await run('rustc', ['--version'], { capture: true })).trim(),
		},
		files: Object.fromEntries(
			await Promise.all(
				files.map(async (file) => [
					file,
					await fileHash(path.join(directory, file)),
				]),
			),
		),
	})
}

/** Refuse missing, stale, changed or unexpected build outputs without compiling. */
export async function verifyCliBuild(directory, identity) {
	const files = await auditDirectory(directory, {
		allowed: buildFileAllowed,
		required: ['build.json', 'dist/index.js', 'THIRD_PARTY_NOTICES.txt'],
	})
	const report = await json(path.join(directory, 'build.json'))
	const actual = files.filter((file) => file !== 'build.json')
	if (
		report.format !== 'bluink.cli-build' ||
		report.formatVersion !== 1 ||
		report.version !== identity.version ||
		report.publicCommit !== identity.publicCommit ||
		report.contractHash !== (await contractHash()) ||
		!report.toolchain ||
		['node', 'npm', 'rust'].some((key) => !report.toolchain[key]) ||
		Object.keys(report).some(
			(key) =>
				![
					'format',
					'formatVersion',
					'version',
					'publicCommit',
					'contractHash',
					'toolchain',
					'files',
				].includes(key),
		) ||
		JSON.stringify(Object.keys(report.files ?? {}).sort()) !==
			JSON.stringify(actual)
	)
		throw new Error('CLI build report does not match this release or contract.')
	publicIdentity(identity.version, {
		publicCommit: identity.publicCommit,
		toolchain: report.toolchain,
	})
	for (const file of actual)
		if (report.files[file] !== (await fileHash(path.join(directory, file))))
			throw new Error(`CLI build output changed: ${file}`)
	return report
}
