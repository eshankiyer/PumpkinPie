// RCON verification suite. A bot connects purely to keep chunks ticking;
// every assertion goes through RCON, since mineflayer cannot parse Pumpkin's
// chunks and its attack packets do not map on this build.
const mineflayer = require('mineflayer')
const { Rcon } = require('rcon-client')
const sleep = ms => new Promise(r => setTimeout(r, ms))
const HOST = '192.168.1.93'

const results = []
function record(name, pass, detail) {
  results.push({ name, pass, detail })
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  (' + detail + ')' : ''}`)
}

async function main () {
  const bot = mineflayer.createBot({ host: HOST, port: 25565, username: 'Verifier', version: '1.21.1', auth: 'offline' })
  bot.on('error', () => {})
  bot.on('kicked', r => console.log('KICKED', JSON.stringify(r)))
  await new Promise(r => bot.once('login', r))
  await sleep(4000)

  const rcon = await Rcon.connect({ host: HOST, port: 25575, password: 'pumpkintest', timeout: 20000 })
  const send = async c => (await rcon.send(c)).trim()
  // `execute if block` echoes the `say` when it matches, so it is our state read.
  const isBlock = async (x, y, z, block) => {
    const r = await send(`execute if block ${x} ${y} ${z} ${block} run say MATCH`)
    return /MATCH/.test(r)
  }

  const [X, Y, Z] = [8, 100, 8]
  await send(`fill ${X - 6} ${Y - 1} ${Z - 6} ${X + 12} ${Y + 6} ${Z + 6} minecraft:air`)
  await send(`fill ${X - 6} ${Y - 1} ${Z - 6} ${X + 12} ${Y - 1} ${Z + 6} minecraft:stone`)
  await send(`tp Verifier ${X} ${Y} ${Z}`)
  await sleep(1500)

  // ---- 1. fire coral bleaches (PR #2571) ----
  await send(`setblock ${X} ${Y} ${Z} minecraft:fire_coral_block`)
  await send(`setblock ${X + 2} ${Y} ${Z} minecraft:brain_coral_block`)
  await sleep(6000)
  const control = await isBlock(X + 2, Y, Z, 'minecraft:dead_brain_coral_block')
  const fire = await isBlock(X, Y, Z, 'minecraft:dead_fire_coral_block')
  record('fire coral bleaches', fire && control, `fire=${fire} controlBrain=${control}`)

  // ---- 2. block_drops gamerule (PR #2575) ----
  await send('gamerule block_drops false')
  await send(`setblock ${X + 4} ${Y} ${Z} minecraft:stone`)
  await send(`setblock ${X + 4} ${Y} ${Z} minecraft:air destroy`)
  await sleep(1200)
  const killedOff = await send(`kill @e[type=minecraft:item,distance=..30]`)
  await send('gamerule block_drops true')
  await send(`setblock ${X + 4} ${Y} ${Z} minecraft:stone`)
  await send(`setblock ${X + 4} ${Y} ${Z} minecraft:air destroy`)
  await sleep(1200)
  const killedOn = await send(`kill @e[type=minecraft:item,distance=..30]`)
  const noneWhenOff = /no |0|not found/i.test(killedOff) || !/[1-9]/.test(killedOff)
  record('block_drops gamerule', noneWhenOff, `off="${killedOff}" on="${killedOn}"`)

  // ---- 3. crop light gate (PR #2567) ----
  await send('gamerule random_tick_speed 1000')
  // Sealed dark box: farmland + water below, wheat inside, lid on top.
  await send(`setblock ${X + 6} ${Y - 1} ${Z} minecraft:farmland`)
  await send(`setblock ${X + 6} ${Y - 2} ${Z} minecraft:water`)
  await send(`setblock ${X + 6} ${Y} ${Z} minecraft:wheat[age=0]`)
  await send(`fill ${X + 5} ${Y + 1} ${Z - 1} ${X + 7} ${Y + 1} ${Z + 1} minecraft:stone`)
  await send(`fill ${X + 5} ${Y} ${Z - 1} ${X + 5} ${Y} ${Z + 1} minecraft:stone`)
  await send(`fill ${X + 7} ${Y} ${Z - 1} ${X + 7} ${Y} ${Z + 1} minecraft:stone`)
  await send(`setblock ${X + 6} ${Y} ${Z - 1} minecraft:stone`)
  await send(`setblock ${X + 6} ${Y} ${Z + 1} minecraft:stone`)
  await sleep(15000)
  const darkGrew = !(await isBlock(X + 6, Y, Z, 'minecraft:wheat[age=0]'))
  record('crops do NOT grow in the dark', !darkGrew, `stillAge0=${!darkGrew}`)

  // Lit control: same rig, torch inside, should still grow.
  await send(`setblock ${X + 10} ${Y - 1} ${Z} minecraft:farmland`)
  await send(`setblock ${X + 10} ${Y - 2} ${Z} minecraft:water`)
  await send(`setblock ${X + 10} ${Y} ${Z} minecraft:wheat[age=0]`)
  await send(`setblock ${X + 10} ${Y} ${Z + 1} minecraft:torch`)
  await sleep(15000)
  const litGrew = !(await isBlock(X + 10, Y, Z, 'minecraft:wheat[age=0]'))
  record('crops DO grow when lit', litGrew, `advanced=${litGrew}`)

  // ---- 4. ice melts in bright light (PR #2579) ----
  await send(`setblock ${X - 2} ${Y} ${Z} minecraft:ice`)
  await send(`setblock ${X - 2} ${Y} ${Z + 1} minecraft:torch`)
  await send(`setblock ${X - 4} ${Y} ${Z} minecraft:ice`) // control, no light source
  await sleep(15000)
  const iceMelted = await isBlock(X - 2, Y, Z, 'minecraft:water')
  const iceControl = await isBlock(X - 4, Y, Z, 'minecraft:ice')
  record('ice melts next to a torch', iceMelted, `melted=${iceMelted} darkControlIntact=${iceControl}`)

  // ---- 5. farmland hydrates from a waterlogged block (PR #2569) ----
  await send(`setblock ${X - 2} ${Y - 1} ${Z + 4} minecraft:farmland[moisture=0]`)
  await send(`setblock ${X} ${Y - 1} ${Z + 4} minecraft:oak_slab[type=bottom,waterlogged=true]`)
  await sleep(12000)
  const hydrated = await isBlock(X - 2, Y - 1, Z + 4, 'minecraft:farmland[moisture=7]')
  record('farmland hydrates from waterlogged block', hydrated, `moisture7=${hydrated}`)

  // ---- 6. fire extinguishes in rain (PR #2569) ----
  await send('weather clear')
  await send(`setblock ${X + 8} ${Y} ${Z + 4} minecraft:netherrack`)
  await send(`setblock ${X + 8} ${Y + 1} ${Z + 4} minecraft:fire`)
  await sleep(4000)
  const burnsWhenClear = await isBlock(X + 8, Y + 1, Z + 4, 'minecraft:fire')
  // Plain stone under it so it is not infiniburn, open to the sky.
  await send(`setblock ${X + 10} ${Y - 1} ${Z + 4} minecraft:stone`)
  await send(`setblock ${X + 10} ${Y} ${Z + 4} minecraft:fire`)
  await send('weather rain')
  await sleep(20000)
  const outInRain = !(await isBlock(X + 10, Y, Z + 4, 'minecraft:fire'))
  record('fire extinguishes in rain', outInRain, `clearControlStillBurning=${burnsWhenClear} rainedOut=${outInRain}`)
  await send('weather clear')

  // ---- 7. grass spreads to dirt (PR #2580) ----
  await send(`fill ${X - 6} ${Y - 1} ${Z - 4} ${X - 2} ${Y - 1} ${Z - 2} minecraft:grass_block`)
  await send(`setblock ${X - 4} ${Y - 1} ${Z - 3} minecraft:dirt`)
  await send('time set day')
  await sleep(25000)
  const spread = await isBlock(X - 4, Y - 1, Z - 3, 'minecraft:grass_block')
  record('grass spreads to dirt', spread, `regrew=${spread}`)

  console.log('\n=== SUMMARY ===')
  for (const r of results) console.log(`${r.pass ? 'PASS' : 'FAIL'}  ${r.name}`)
  console.log(`${results.filter(r => r.pass).length}/${results.length} passed`)
  rcon.end(); bot.quit(); process.exit(0)
}
main().catch(e => { console.error('FATAL', e.message); process.exit(1) })
