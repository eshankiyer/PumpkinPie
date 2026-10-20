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
  const probe = async (x, y, z) => {
    const r = await send(`setblock ${x} ${y} ${z} minecraft:glass keep`)
    const wasAir = /placed/i.test(r)
    if (wasAir) await send(`setblock ${x} ${y} ${z} minecraft:air`)
    return wasAir ? 'AIR' : 'OCCUPIED'
  }
  for (const g of ['gamerule random_tick_speed 300', 'gamerule randomTickSpeed 300']) {
    console.log(g, '=>', await send(g))
  }
  const Y = Math.floor(bot.entity.position.y), Z = Math.floor(bot.entity.position.z) + 4, XL = Math.floor(bot.entity.position.x) + 4
  await send(`setblock ${XL} ${Y} ${Z} minecraft:oak_leaves`)
  await send(`setblock ${XL + 2} ${Y} ${Z} minecraft:oak_log`)
  await send(`setblock ${XL + 3} ${Y} ${Z} minecraft:oak_leaves`)
  for (let t = 15; t <= 210; t += 15) {
    await sleep(15000)
    const lone = await probe(XL, Y, Z)
    const near = await probe(XL + 3, Y, Z)
    console.log(`t=${t}s lone=${lone} nearLog=${near}`)
    if (lone === 'AIR') break
  }
  rcon.end(); bot.quit(); process.exit(0)
}
main().catch(e => { console.error('FATAL', e.message); process.exit(1) })
