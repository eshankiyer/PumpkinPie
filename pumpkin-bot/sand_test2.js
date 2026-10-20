// RCON-only repro for #2467: bot connects only to keep chunks loaded/ticking;
// block states are probed server-side with `fill ... replace` result counts.
const mineflayer = require('mineflayer')
const { Rcon } = require('rcon-client')

const HOST = process.env.PUMPKIN_HOST || '192.168.1.93'
const sleep = ms => new Promise(r => setTimeout(r, ms))

async function main () {
  const bot = mineflayer.createBot({
    host: HOST, port: 25565, username: 'TestBot', version: process.env.MC_VERSION || '1.21.1', auth: 'offline'
  })
  bot.on('kicked', r => { console.log('KICKED:', JSON.stringify(r)); process.exit(2) })
  bot.on('error', () => {}) // chunk parse errors are expected and irrelevant
  await new Promise(res => bot.once('login', res))
  console.log('bot logged in (chunk loader only)')
  await sleep(4000)

  const rcon = await Rcon.connect({ host: HOST, port: 25575, password: 'pumpkintest' })
  const send = async c => {
    const r = (await rcon.send(c)).trim()
    console.log('>', c, '=>', r || '(empty)')
    return r
  }
  // Probe with `setblock keep`: it only places into air, so the response
  // tells us whether the position was air (then we restore it) or occupied.
  const probe = async (x, y, z, label) => {
    const r = await send(`setblock ${x} ${y} ${z} minecraft:glass keep`)
    const wasAir = /placed/i.test(r)
    if (wasAir) await send(`setblock ${x} ${y} ${z} minecraft:air`)
    console.log(`   [${label}] ${x},${y},${z}: ${wasAir ? 'AIR' : 'OCCUPIED'}`)
    return wasAir
  }

  // Fixed rig location near spawn, high enough to be in loaded chunks.
  const X = 8, Z = 8, Y = 120
  await send(`setblock ${X} ${Y} ${Z} minecraft:stone`)
  await send(`setblock ${X} ${Y + 1} ${Z} minecraft:stone`)
  await send(`setblock ${X} ${Y + 2} ${Z} minecraft:sand`)
  await sleep(1000)
  console.log('--- before support removal ---')
  await probe(X, Y + 2, Z, 'sand start pos')

  await send(`setblock ${X} ${Y + 1} ${Z} minecraft:air`)
  for (const wait of [2000, 3000, 5000]) {
    await sleep(wait)
    console.log(`--- ${wait}ms later ---`)
    await probe(X, Y + 2, Z, 'start pos (expect AIR after fall)')
    await probe(X, Y + 1, Z, 'landing pos (expect OCCUPIED=sand)')
  }
  // Rig B: sand placed directly above an air gap. This exercises the
  // placed() -> scheduled tick -> falling entity path on its own.
  const X2 = 12
  await send(`setblock ${X2} ${Y} ${Z} minecraft:stone`)
  await send(`setblock ${X2} ${Y + 1} ${Z} minecraft:air`)
  await send(`setblock ${X2} ${Y + 2} ${Z} minecraft:air`)
  await send(`setblock ${X2} ${Y + 3} ${Z} minecraft:sand`)
  for (const wait of [2000, 3000, 5000]) {
    await sleep(wait)
    console.log(`--- rig B: ${wait}ms later ---`)
    await probe(X2, Y + 3, Z, 'B start (expect AIR)')
    await probe(X2, Y + 2, Z, 'B mid (expect AIR)')
    await probe(X2, Y + 1, Z, 'B landing (expect OCCUPIED=sand)')
  }
  rcon.end()
  bot.quit()
  process.exit(0)
}

main().catch(e => { console.error('FATAL', e.message); process.exit(1) })
