import { mkdir, readFile } from 'node:fs/promises'
import { parseArgs } from 'node:util'
import path from 'node:path'
import os from 'node:os'
import { fileURLToPath } from 'node:url'
import {
	nativeBuildInfo,
	nativeSourceDigest,
	npm,
	root,
	run,
	targets,
	writeJson,
} from './release-utils.mjs'

export async function buildNative(targetName, cross = false) {
	const target = targets.find((value) =>
		targetName
			? value.rust === targetName
			: value.platform === process.platform && value.arch === process.arch,
	)
	if (!target)
		throw new Error(
			`Unsupported native build target: ${targetName ?? `${process.platform}/${process.arch}`}`,
		)
	const sourceDigest = await nativeSourceDigest()
	const version = JSON.parse(
		await readFile(path.join(root, 'backend/package.json'), 'utf8'),
	).version
	const publicCommit = process.env.BLUINK_PUBLIC_COMMIT
	if (
		publicCommit &&
		publicCommit !==
			(await run('git', ['rev-parse', 'HEAD'], { capture: true })).trim()
	)
		throw new Error('Public build commit does not match this checkout.')
	const info = JSON.stringify({
		sourceDigest,
		target: target.rust,
		version,
		...(publicCommit && { publicCommit }),
	})
	const env = { ...process.env, BLUINK_NATIVE_BUILD_INFO: info }
	const inherited =
		env.CARGO_ENCODED_RUSTFLAGS ??
		(env.RUSTFLAGS ?? '').split(/\s+/).filter(Boolean).join('\u001f')
	env.CARGO_ENCODED_RUSTFLAGS = [
		inherited,
		`--remap-path-prefix=${root}=/bluink`,
		`--remap-path-prefix=${process.env.CARGO_HOME ?? path.join(os.homedir(), '.cargo')}=/cargo`,
		`--remap-path-prefix=${process.env.RUSTUP_HOME ?? path.join(os.homedir(), '.rustup')}=/rust`,
	]
		.filter(Boolean)
		.join('\u001f')
	if (cross && target.platform === 'darwin' && process.platform !== 'darwin') {
		// Only a link-time declaration: macOS supplies the actual framework.
		const flags =
			env.CARGO_ENCODED_RUSTFLAGS ??
			(env.RUSTFLAGS ?? '').split(/\s+/).filter(Boolean).join('\u001f')
		env.CARGO_ENCODED_RUSTFLAGS = [
			flags,
			'-L',
			`framework=${path.join(root, 'backend/runtime/scripts/darwin-frameworks')}`,
		]
			.filter(Boolean)
			.join('\u001f')
	}
	if (cross && target.platform === 'win32' && process.platform !== 'win32') {
		env.PATH = `${path.join(root, 'backend/runtime/scripts/llvm-tools')}${path.delimiter}${env.PATH ?? ''}`
		env.TARGET_CC = 'clang'
	}
	await npm(
		[
			'exec',
			'--workspace',
			'backend',
			'--',
			'napi',
			'build',
			'--platform',
			'--release',
			'--manifest-path',
			'Cargo.toml',
			'--package-json-path',
			'package.json',
			'--output-dir',
			'runtime',
			'--no-js',
			'--target',
			target.rust,
			...(cross ? ['--cross-compile'] : []),
		],
		{ env },
	)
	if (sourceDigest !== (await nativeSourceDigest()))
		throw new Error(
			'Native sources changed while compiling. Rebuild before releasing.',
		)
	const artifact = path.join(root, 'backend/runtime', target.artifact)
	const compiled = nativeBuildInfo(await readFile(artifact))
	if (compiled.sourceDigest !== sourceDigest || compiled.target !== target.rust)
		throw new Error(
			'The compiler did not produce the requested native build identity.',
		)
	await mkdir(path.dirname(artifact), { recursive: true })
	await writeJson(`${artifact}.json`, compiled)
	console.log(`Built ${target.artifact} from ${sourceDigest.slice(0, 12)}`)
}

if (
	process.argv[1] &&
	path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	const { values } = parseArgs({
		options: {
			target: { type: 'string' },
			cross: { type: 'boolean', default: false },
		},
	})
	await buildNative(values.target, values.cross)
}
