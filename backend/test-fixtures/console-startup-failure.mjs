// Explicit PTY acceptance fixture; never imported by the product launcher.
import { createRequire } from 'node:module'
import { realpathSync } from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const cli = realpathSync(
	process.env.BLUINK_CONSOLE_CLI ??
		fileURLToPath(new URL('../bluink', import.meta.url)),
)
const { BluinkApplication } = createRequire(import.meta.url)(
	path.join(path.dirname(cli), 'runtime/index.cjs'),
)
BluinkApplication.prototype.startHttp = async () => {
	await new Promise((resolve) => setTimeout(resolve, 500))
	throw new Error('Controlled startup failure')
}
