// Verification for the setblock shape-update fix and leaves distance/decay.
const mineflayer = require('mineflayer')
const { Rcon } = require('rcon-client')

const HOST = process.env.PUMPKIN_HOST || '192.168.1.93'
const sleep = ms => new Promise(r => setTimeout(r, ms))

async function main () {
  const bot = mineflayer.createBot({
    host: HOST, port: 25565, username: 'TestBot', version: '1.21.1', auth: 'offline'
  })
  bot.on('kicked', r => { console.log('KICKED:', JSON.stringify(r)); process.exit(2) })
  bot.on('error', () => {})
  await new Promise(res => bot.once('login', res))
  console.log('bot logged in')
  await sleep(4000)

  const rcon = await Rcon.connect({ host: HOST, port: 25575, password: 'pumpkintest' })
  const send = async c => (await rcon.send(c)).trim()
  const probe = async (x, y, z) => {
    const r = await send(`setblock ${x} ${y} ${z} minecraft:glass keep`)
    const wasAir = /placed/i.test(r)
    if (wasAir) await send(`setblock ${x} ${y} ${z} minecraft:air`)
    return wasAir ? 'AIR' : 'OCCUPIED'
  }

  // TEST 1 (setblock fix): support removal must make sand fall.
  const X = 20, Z = 8, Y = 120
  await send(`setblock ${X} ${Y} ${Z} minecraft:stone`)
  await send(`setblock ${X} ${Y + 1} ${Z} minecraft:stone`)
  await send(`setblock ${X} ${Y + 2} ${Z} minecraft:sand`)
  await sleep(1000)
  await send(`setblock ${X} ${Y + 1} ${Z} minecraft:air`)
  await sleep(4000)
  const startPos = await probe(X, Y + 2, Z)
  const landPos = await probe(X, Y + 1, Z)
  console.log(`TEST1 sand-falls-on-setblock: start=${startPos} landing=${landPos} => ` +
    (startPos === 'AIR' && landPos === 'OCCUPIED' ? 'PASS' : 'FAIL'))

  // TEST 2 (leaves): lone leaves decay, leaves next to a log survive.
  await send('gamerule randomTickSpeed 300').then(r => console.log('gamerule:', r || '(empty)'))
  const XL = 24
  await send(`setblock ${XL} ${Y} ${Z} minecraft:oak_leaves`)        // lone
  await send(`setblock ${XL + 2} ${Y} ${Z} minecraft:oak_log`)       // log
  await send(`setblock ${XL + 3} ${Y} ${Z} minecraft:oak_leaves`)    // next to log
  for (const wait of [10000, 20000, 30000]) {
    await sleep(wait)
    const lone = await probe(XL, Y, Z)
    const nearLog = await probe(XL + 3, Y, Z)
    console.log(`TEST2 t+=${wait / 1000}s lone=${lone} nearLog=${nearLog}`)
    if (lone === 'AIR') {
      console.log(`TEST2 leaves-decay: ${nearLog === 'OCCUPIED' ? 'PASS (lone decayed, log-adjacent survived)' : 'PARTIAL: lone decayed but log-adjacent also gone'}`)
      break
    }
  }
  rcon.end()
  bot.quit()
  process.exit(0)
}

main().catch(e => { console.error('FATAL', e.message); process.exit(1) })
