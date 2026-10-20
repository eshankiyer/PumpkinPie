// Reproduction harness for Pumpkin issue #2467 (sand disappears after falling).
// Connects a mineflayer bot as an observer, uses RCON to build a test rig,
// removes the support block, then samples the world until the sand settles.
const mineflayer = require('mineflayer')
const { Rcon } = require('rcon-client')

const HOST = process.env.PUMPKIN_HOST || '192.168.1.93'
const RCON_PASS = process.env.RCON_PASS || 'pumpkintest'
const VERSION = process.env.MC_VERSION || '1.21.1'
const sleep = ms => new Promise(r => setTimeout(r, ms))

async function main () {
  const bot = mineflayer.createBot({
    host: HOST, port: 25565, username: 'TestBot', version: VERSION, auth: 'offline'
  })
  bot.on('kicked', r => { console.log('KICKED:', JSON.stringify(r)); process.exit(2) })
  bot.on('error', e => { console.log('BOT ERROR:', e.message) })
  await new Promise(res => bot.once('spawn', res))
  console.log('spawned at', bot.entity.position)
  await sleep(3000) // let chunks stream in

  const rcon = await Rcon.connect({ host: HOST, port: 25575, password: RCON_PASS })
  const p = bot.entity.position.floored()
  const x = p.x + 3, z = p.z + 3, y = p.y
  const at = dy => bot.blockAt(p.offset(3, dy, 3))
  const name = dy => (at(dy) ? at(dy).name : '?')

  // Rig: stone base at y, stone support at y+1, sand at y+2.
  for (const c of [
    `setblock ${x} ${y} ${z} minecraft:stone`,
    `setblock ${x} ${y + 1} ${z} minecraft:stone`,
    `setblock ${x} ${y + 2} ${z} minecraft:sand`
  ]) console.log('>', c, '->', await rcon.send(c))
  await sleep(1500)
  console.log(`BEFORE: y+2=${name(2)} y+1=${name(1)} y=${name(0)}`)

  // Remove the support. Sand at y+2 should fall and settle at y+1.
  console.log('> remove support ->', await rcon.send(`setblock ${x} ${y + 1} ${z} minecraft:air`))
  for (let t = 1; t <= 12; t++) {
    await sleep(500)
    console.log(`t=${(t * 0.5).toFixed(1)}s  y+2=${name(2)}  y+1=${name(1)}  y=${name(0)}`)
  }

  const settled = name(1)
  console.log(settled === 'sand'
    ? 'RESULT: OK - sand settled on the block beneath'
    : `RESULT: BUG REPRODUCED - expected sand at y+1, found ${settled}`)
  rcon.end()
  bot.quit()
  process.exit(0)
}

main().catch(e => { console.error('FATAL', e); process.exit(1) })
