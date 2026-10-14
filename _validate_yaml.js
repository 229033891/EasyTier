import fs from 'fs'
import path from 'path'
import yaml from 'js-yaml'
import { createRequire } from 'module'

const require = createRequire(import.meta.url)
const locales = 'D:/EasyTier/easytier-web/frontend-lib/src/locales'
for (const f of fs.readdirSync(locales).filter((x) => x.endsWith('.yaml'))) {
  const p = path.join(locales, f)
  try {
    yaml.load(fs.readFileSync(p, 'utf8'))
    console.log('OK', f)
  } catch (e) {
    console.log('FAIL', f, e.message)
  }
}
