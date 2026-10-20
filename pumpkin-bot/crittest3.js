// Crit verification using a REAL fall: RCON-teleport the attacker up and let
// mineflayer emit its own protocol-correct position packets while descending,
// so the server accumulates fall_distance the normal way. Entity ids and
// animations are read straight off the wire because mineflayer cannot parse
// Pumpkin's chunks.
const mineflayer = require('mineflayer')
const { Rcon } = require('rcon-client')
const sleep = ms => new Promise(r => setTimeout(r, ms))
const mk = n => mineflayer.createBot({ host: '192.168.1.93', port: 25565, username: n, version: '1.21.1', auth: 'offline' })

async function main () {
  const obs = mk('Observer'); obs.on('error', () => {})
  await new Promise(r => obs.once('login', r))
  const atk = mk('Attacker'); atk.on('error', () => {})
  await new Promise(r => atk.once('login', r))
  await sleep(4000)

  const anims = []
  for (const b of [obs, atk]) {
    b._client.on('animation', p => {
      if (p.animation === 4 || p.animation === 5) {
        anims.push({ who: b.username, id: p.animation, ent: p.entityId })
        console.log(`ANIM ${b.username} id=${p.animation} entity=${p.entityId}`)
      }
    })
  }
  let cowId = null
  atk._client.on('spawn_entity', p => { if (cowId === null) cowId = p.entityId })

  const rcon = await Rcon.connect({ host: "192.168.1.93", port: 25575, password: "pumpkintest", timeout: 15000 })
  const send = async c => (await rcon.send(c)).trim()
  await send('gamerule doImmediateRespawn true')

  // The bot has no chunk data so it free-falls; park both bots on a built
  // platform first, otherwise they drift out of entity-tracking range.
  const [x, y, z] = [8, 100, 8]
  await send(`fill ${x - 3} ${y - 1} ${z - 3} ${x + 3} ${y - 1} ${z + 3} minecraft:stone`)
  await send(`tp Attacker ${x - 1} ${y} ${z}`).catch(()=>{})
  await send(`tp Observer ${x + 2} ${y} ${z}`).catch(()=>{})
  await sleep(1500)
  console.log('summon:', await send(`summon minecraft:cow ${x} ${y} ${z}`))
  await sleep(3000)
  if (cowId === null) { console.log('RESULT: INCONCLUSIVE no spawn_entity seen'); process.exit(1) }
  console.log('cow entity id:', cowId)

  for (let i = 0; i < 14; i++) {
    // Real fall: teleport up, then swing on the way down.
    await send(`tp Attacker ${x - 1} ${y + 6} ${z}`)
    await sleep(430) // descending, not yet landed
    atk._client.write('use_entity', { target: cowId, mouse: 1, sneaking: false })
    atk.swingArm('right')
    await sleep(950)
  }
  await sleep(2000)

  const crit = anims.filter(a => a.id === 4 && a.ent === cowId)
  console.log(`RESULT: ${crit.length > 0 ? 'PASS' : 'FAIL'} critAnims=${crit.length} observerSaw=${crit.filter(c => c.who === 'Observer').length} allAnims=${anims.length}`)
  rcon.end(); obs.quit(); atk.quit(); process.exit(0)
}
main().catch(e => { console.error('FATAL', e.message); process.exit(1) })
