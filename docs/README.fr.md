<div align="center">

[English](../README.md) · **Français**

<img src="../resources/icon.svg" alt="Cachemire" width="128" height="128">

# Cachemire

**Votre cache sous toutes les coutures !**

Pensé pour les hashs Redis remplis de JSON : lisez chaque valeur en JSON coloré, filtrez-les toutes avec un chemin JSON et affichez les résultats dans un tableau exportable en CSV. Redisql, un petit langage de requête inspiré du SQL, extrait des lignes de plusieurs clés et les joint entre elles. Fonctionne à travers des tunnels SSH et avec AWS ElastiCache.

<p>
  <a href="https://github.com/iRed4321/Cachemire/releases/latest"><img alt="Version stable" src="https://img.shields.io/github/v/release/iRed4321/Cachemire?label=stable"></a>
  <a href="https://github.com/iRed4321/Cachemire/releases"><img alt="Dernière version" src="https://img.shields.io/github/v/release/iRed4321/Cachemire?include_prereleases&label=latest"></a>
  <a href="https://github.com/iRed4321/Cachemire/releases"><img alt="Téléchargements" src="https://img.shields.io/github/downloads/iRed4321/Cachemire/total"></a>
  <a href="https://github.com/iRed4321/Cachemire/actions/workflows/release.yml"><img alt="Build de release" src="https://img.shields.io/github/actions/workflow/status/iRed4321/Cachemire/release.yml"></a>
  <a href="../LICENSE"><img alt="Licence" src="https://img.shields.io/github/license/iRed4321/Cachemire"></a>
  <br>
  <a href="https://www.rust-lang.org"><img alt="Rust" src="https://img.shields.io/badge/Rust-dea584?logo=rust&logoColor=black"></a>
  <a href="https://slint.dev"><img alt="Slint" src="https://img.shields.io/badge/UI-Slint-2379f4"></a>
  <img alt="Plateformes : Windows et Linux" src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux-informational">
</p>

</div>

---

*Traduction du [README anglais](../README.md), qui fait foi.*

<p align="center">
  <img src="screenshots/table-view.png" alt="Un hash de valeurs JSON affiché en tableau, une colonne par clé JSON" width="45%">
  &nbsp;
  <img src="screenshots/fields-view.png" alt="Le même genre de hash en champs, dont un déplié en JSON indenté" width="45%">
</p>

> **Construit autour de mon propre usage.** Les fonctionnalités et types de Redis ne sont pas encore tous pris en charge : le reste arrivera progressivement.

## Fonctionnalités

### Parcourir et lire

- 🌳 **Arbre des clés** construit à partir des `:` dans les noms de clés (`app:users:42` devient `app` › `users` › `42`). Il se charge en arrière-plan : vous pouvez l'utiliser immédiatement, il se complète au fil du chargement. Cherchez en texte simple ou avec les motifs Redis (`user:*`).
- 📦 **Tous les types Redis** : chaîne, hash, liste, ensemble, ensemble trié et stream, avec le type et le TTL affichés dans l'en-tête de la clé. Les gros hashs se chargent page par page, au fil du défilement.
- 🎨 **JSON coloré** : une ligne par champ quand il est replié, mis en forme quand il est déplié. Sélectionnez et copiez du texte, ou copiez, ouvrez et téléchargez une valeur en JSON.
- 🔍 **Recherche dans les champs** : par nom, par valeur, ou les deux. Une recherche de texte permet aussi de retrouver un passage dans une valeur ouverte.

### Filtrer et afficher en tableau

Saisissez un chemin JSON commençant par `$` dans la boîte de recherche de la clé, puis appuyez sur <kbd>Entrée</kbd> :

```
$.records               une clé
$.records.*.name        le nom de chaque enregistrement
$.records[0].address.city un indice de tableau
$..label                une clé à n'importe quelle profondeur
```

Le chemin est complété à partir des valeurs chargées. Affichez le résultat sous forme de **liste champ/valeur** ou de **tableau** aux colonnes redimensionnables, copiez les lignes cochées dans un tableau JSON, ou exportez-les en CSV.

### Chercher et interroger plusieurs clés

- 🧭 **Recherche globale** : un mot ou un motif est comparé au nom, aux noms de champs et aux valeurs de toutes les clés de l'instance.
- 🧮 **Redisql**, un petit langage de requête qui extrait des lignes des clés contenant du JSON et les joint comme en SQL :

  ```
  FROM KEY 'orders:items' AS i
  JOIN KEY 'products' AS p ON i.productId = p.id
  WHERE i.qty IN [1, 2, 3]
  SELECT i.productId, p.label, i.qty
  LIMIT 100
  ```

  L'éditeur signale les erreurs à l'endroit exact où elles se trouvent, complète les mots-clés, les clés et les `alias.champ`, conserve l'historique des exécutions et permet d'enregistrer des requêtes nommées. Le résultat s'ouvre comme n'importe quelle clé : vues, filtres et exports s'appliquent directement.

### Modifier

- ✏️ **Éditeur de valeurs** : le crayon d'une ligne ouvre sa valeur en JSON, avec numéros de ligne et couleurs, et l'enregistre dans Redis dès qu'elle est valide. Une clé de type chaîne est réécrite en entier, son TTL conservé ; les champs d'un hash peuvent être ajoutés, renommés et supprimés depuis la même fenêtre.
- 🗂️ **Clés et dossiers** : un clic droit dans l'arbre permet d'ajouter une clé, ou de renommer ou supprimer une clé ou tout un dossier.

### Onglets

Les clés, les recherches et les requêtes s'ouvrent dans des onglets que vous pouvez réorganiser par glisser-déposer. Chaque onglet conserve ses filtres et sa position de défilement, et chaque connexion garde son propre ensemble d'onglets.

### Se connecter

- **URL de connexion** : `redis://user:pass@host:6379/0`, ou `rediss://` pour le TLS. Testez-la avant de l'enregistrer.
- 🔐 **Tunnels SSH** par mot de passe, par clé privée, ou les deux. La clé d'hôte est présentée pour approbation la première fois (ou reprise de `~/.ssh/known_hosts`), puis refusée si elle change. Les profils SSH permettent de partager un même identifiant entre plusieurs tunnels.
- 🎛️ **Profils de connexion** : un nom et une couleur qui teinte l'application pendant la connexion, pour toujours savoir si vous êtes sur `Production`.
- ☁️ **AWS** : connectez-vous avec vos profils locaux `~/.aws` (SSO compris) et listez les points de terminaison ElastiCache de chacun.
- 📥 **Import / export** de vos connexions au format JSON, avec ou sans les mots de passe.
- 🔗 **Liens partageables** : le bouton de lien à côté du nom d'une clé copie un lien `cachemire://` vers la clé, avec son filtre ou l'un de ses champs, sans jamais contenir d'identifiant ni de mot de passe. L'ouvrir mène à cette clé sur la connexion enregistrée ou le point de terminaison AWS correspondant. L'installeur Windows et le .deb enregistrent ces liens ; un AppImage ne le fait qu'une fois intégré au bureau.

> Le mode cluster n'est pas encore pris en charge : un serveur de ce type est refusé avec un message explicite.

### Sûr par défaut

- 🛡️ Les mots de passe sont stockés dans le trousseau du système (Gestionnaire d'identification ou Secret Service). S'il est indisponible, Cachemire demande avant de garder un mot de passe dans son fichier de données, et peut à la place enregistrer sans lui et le demander à la connexion.
- 🔑 La clé d'hôte d'un serveur SSH n'est jamais approuvée en silence : son empreinte vous est présentée d'abord, les clés de `~/.ssh/known_hosts` sont respectées, et une clé qui change est refusée.
- 🔒 **Profils et connexions en lecture seule** : cochez *Lecture seule* sur un profil de connexion, ou sur une seule connexion, et rien ne peut être écrit dans Redis par ce biais. Le profil `Production` par défaut est en lecture seule.
- La suppression d'un dossier demande d'abord confirmation, en indiquant le nombre de clés qui seront supprimées.
- Une valeur qui a changé dans Redis pendant que vous la modifiiez n'est jamais écrasée silencieusement : vous choisissez d'annuler, de la recharger ou de l'écraser.

### Finitions

Zoomez avec <kbd>Ctrl</kbd>+<kbd>+</kbd>/<kbd>-</kbd>/<kbd>0</kbd> ou <kbd>Ctrl</kbd>+molette. ⌨️ **Raccourcis clavier configurables**. 🌍 **Anglais et français**, selon la langue du système ou votre choix dans les **Paramètres**, avec changement à la volée, sans redémarrage.

## Prévu

- 🗝️ Prise en charge de **Valkey**
- 🎨 **Thèmes**
- 🖍️ **Coloration syntaxique** des valeurs dans d'autres langages que le JSON

## Installer

Téléchargez le fichier correspondant à votre système sur la [page des versions](https://github.com/iRed4321/Cachemire/releases/latest) :

| Système | Fichier | Description |
|---|---|---|
| Windows | `Cachemire-<version>-x64.msi` | l'installeur : entrée du menu Démarrer, liens `cachemire://`, mises à jour |
| Windows | `Cachemire-Portable-<version>-x64.zip` | l'exécutable portable : décompressez-le et lancez-le |
| Linux (Debian, Ubuntu) | `cachemire_<version>-1_amd64.deb` | à installer avec `sudo apt install ./cachemire_<version>-1_amd64.deb` |
| Linux (toute distribution) | `Cachemire-<version>-x86_64.AppImage` | rendez-le exécutable (`chmod +x`) puis lancez-le |

**Mises à jour** : sous Windows, **Paramètres › À propos** cherche une nouvelle version, puis télécharge et lance son installeur. Sous Linux, il ouvre la page de la version, d'où vous téléchargez le nouveau fichier.

## Compiler depuis les sources

Il vous faut un **Rust** stable récent. Sous Linux, il faut également un Secret Service actif et Wayland ou X11.

```sh
cargo run --release
```

Ajoutez une connexion depuis la barre de titre (**Nouvelle connexion…**), sélectionnez-la, puis parcourez vos clés. Pour essayer en local : `docker run --rm -p 6379:6379 redis:7`, puis ajoutez `redis://127.0.0.1:6379`.

| Commande | Usage |
|---|---|
| `cargo run` | debug : compilation la plus rapide, exécution la plus lente |
| `cargo run --profile fast` | optimisé, sans LTO : recompile en quelques secondes, idéal pour essayer l'application réelle |
| `cargo run --release` | avec LTO, sans symboles : la version à distribuer |

`cargo test` lance les tests unitaires et ceux de l'interface sans fenêtre. `cargo xtask e2e` lance les tests de bout en bout : l'application et son cœur face à un serveur Redis jetable, un `redis-server` (ou `valkey-server`) local s'il y en a un, sinon le `redis:7.4` de Docker, sinon sous Windows le `redis-server` de WSL. Chaque test prend une base vidée rien que pour lui.

Les installeurs se construisent avec `cargo xtask build-windows` (MSI et zip portable), `cargo xtask build-deb` et `cargo xtask build-appimage` (ou les deux à partir d'une seule compilation avec `cargo xtask build-linux`).

Pour essayer le MSI seul, `cargo xtask build-installer-test --version 1.2.0` y met une application factice, rapide à compiler, à la version voulue ; sur GitHub, lancez le workflow *Installer test* et téléchargez son artefact.

## Vos données

Elles sont enregistrées dans le dossier de données de l'utilisateur (`%APPDATA%\Cachemire` ou `~/.local/share/Cachemire`) : connexions, réglages, requêtes enregistrées, clés d'hôte SSH et historique des requêtes. Le zip portable de Windows garde tout dans un dossier `data` à côté de l'exécutable.

## À propos du projet

La majeure partie du code de Cachemire a été écrite par une IA ([Claude Code](https://claude.com/claude-code)), sous ma direction et ma relecture.

## Licence

Cachemire est un logiciel libre sous [GNU GPL v3](../LICENSE). Vous pouvez l'utiliser et le forker librement ; si vous redistribuez une version, elle doit rester sous la même licence, avec son code source et une mention de l'auteur d'origine.

Polices intégrées : [Roboto](../resources/fonts/Roboto-OFL.txt) et [JetBrains Mono](../resources/fonts/JetBrainsMono-OFL.txt), sous licence SIL Open Font License.
