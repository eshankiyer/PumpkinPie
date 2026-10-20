// Crit-animation verification at raw protocol level. mineflayer cannot parse
// Pumpkin's chunks, so entity tracking is unusable; we read spawn_entity and
// animation packets directly and send use_entity ourselves.
const mineflayer = require('mineflayer')
const { Rcon } = require('rcon-client')
const sleep = ms => new Promise(r => setTimeout(r, ms))
const mk = n => mineflayer.createBot({ host: '192.168.1.93', port: 25565, username: n, version: '1.21.1', auth: 'offline' })

async function main () {
  const observer = mk('Observer'); observer.on('error', () => {})
  await new Promise(r => observer.once('login', r))
  const atk = mk('Attacker'); atk.on('error', () => {})
  await new Promise(r => atk.once('login', r))
  await sleep(3500)

  const anims = []
  for (const b of [observer, atk]) {
    b._client.on('animation', p => {
      if (p.animation === 4 || p.animation === 5) {
        anims.push({ who: b.username, id: p.animation, ent: p.entityId })
        console.log(`ANIM ${b.username} id=${p.animation} entity=${p.entityId}`)
      }
    })
  }

  // Capture the cow's entity id straight off the wire.
  let cowId = null
  atk._client.on('spawn_entity', p => { if (cowId === null) cowId = p.entityId })

  const rcon = await Rcon.connect({ host: '192.168.1.93', port: 25575, password: 'pumpkintest' })
  const pos = atk.entity.position
  const [x, y, z] = [Math.floor(pos.x) + 1, Math.floor(pos.y), Math.floor(pos.z)]
  console.log('summon:', (await rcon.send(`summon minecraft:cow ${x} ${y} ${z}`)).trim())
  await sleep(2500)
  if (cowId === null) { console.log('RESULT: INCONCLUSIVE no spawn_entity packet seen'); process.exit(1) }
  console.log('cow entity id (from packet):', cowId)

  // A crit needs the server to see us airborne with fall distance > 0, so we
  // drive position packets ourselves: hop up, then attack while descending.
  for (let attempt = 0; attempt < 10; attempt++) {
    const bx = pos.x, bz = pos.z
    for (const dy of [1.2, 0.9, 0.6, 0.35]) {
      atk._client.write('position', { x: bx, y: pos.y + dy, z: bz, onGround: false })
      await sleep(60)
    }
    atk._client.write('use_entity', { target: cowId, mouse: 1, sneaking: false })
    atk._client.write('arm_animation', { hand: 0 })
    await sleep(120)
    atk._client.write('position', { x: bx, y: pos.y, z: bz, onGround: true })
    await sleep(700)
  }
  await sleep(1500)

  const crit = anims.filter(a => a.id === 4 && a.ent === cowId)
  const byObs = crit.filter(a => a.who === 'Observer').length
  console.log(`RESULT: ${crit.length > 0 ? 'PASS' : 'FAIL'} crit anims=${crit.length} observerSaw=${byObs} totalAnims=${anims.length}`)
  rcon.end(); observer.quit(); atk.quit(); process.exit(0)
}
main().catch(e => { console.error('FATAL', e.message); process.exit(1) })
