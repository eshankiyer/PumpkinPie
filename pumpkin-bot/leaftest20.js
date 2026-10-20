// 20 isolated leaf blocks: P(all survive if decay works) is astronomically small,
// so this cleanly separates "code not running" from "unlucky sampling".
const mineflayer = require('mineflayer')
const { Rcon } = require('rcon-client')
const sleep = ms => new Promise(r => setTimeout(r, ms))
async function main () {
  const bot = mineflayer.createBot({ host: '192.168.1.93', port: 25565, username: 'TestBot', version: '1.21.1', auth: 'offline' })
  bot.on('error', () => {})
  await new Promise(res => bot.once('login', res))
  await sleep(3000)
  const rcon = await Rcon.connect({ host: '192.168.1.93', port: 25575, password: 'pumpkintest' })
  const send = async c => (await rcon.send(c)).trim()
  const isAir = async (x, y, z) => {
    const r = await send(`setblock ${x} ${y} ${z} minecraft:glass keep`)
    const air = /placed/i.test(r)
    if (air) await send(`setblock ${x} ${y} ${z} minecraft:air`)
    return air
  }
  const p = bot.entity.position
  const Y = Math.floor(p.y), Z = Math.floor(p.z) + 5, X0 = Math.floor(p.x) - 10
  // 20 lone leaves spaced 2 apart, plus one control pair (log + adjacent leaves)
  for (let i = 0; i < 20; i++) await send(`setblock ${X0 + i * 2} ${Y} ${Z} minecraft:oak_leaves`)
  await send(`setblock ${X0} ${Y} ${Z + 4} minecraft:oak_log`)
  await send(`setblock ${X0 + 1} ${Y} ${Z + 4} minecraft:oak_leaves`)
  console.log(`placed 20 lone leaves at y=${Y} z=${Z}, control log+leaf at z=${Z + 4}`)
  for (let t = 30; t <= 240; t += 30) {
    await sleep(30000)
    let gone = 0
    for (let i = 0; i < 20; i++) if (await isAir(X0 + i * 2, Y, Z)) gone++
    const control = await isAir(X0 + 1, Y, Z + 4)
    console.log(`t=${t}s decayed=${gone}/20 controlLeafGone=${control}`)
    if (gone >= 15) { console.log(`RESULT: PASS decay works (${gone}/20), control survived=${!control}`); break }
    if (t === 240) console.log(`RESULT: FAIL only ${gone}/20 decayed`)
  }
  rcon.end(); bot.quit(); process.exit(0)
}
main().catch(e => { console.error('FATAL', e.message); process.exit(1) })
