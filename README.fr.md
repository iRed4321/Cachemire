<div align="center">

[English](README.md) · **Français**

<img src="assets/icon.svg" alt="Cachemire" width="128" height="128">

# Cachemire

**Votre cache sous toutes les coutures.**

Un explorateur de clés Redis rapide et natif. Parcourez vos clés en arbre, lisez vos valeurs en JSON coloré, filtrez-les avec un chemin JSON et affichez-les en tableau. Accédez à des serveurs derrière un bastion SSH, avec tous vos mots de passe rangés dans le trousseau de votre système.

Rust · [Slint](https://slint.dev) · Windows et Linux

</div>

---

*Traduction du [README anglais](README.md), qui fait foi.*

> **Pensé pour un usage particulier.** Cachemire est conçu autour des hashs dont les valeurs sont du JSON. C'est là que ses filtres, ses tableaux et Redisql sont les plus utiles ; les autres types restent lisibles, mais ont reçu moins d'attention.

## Fonctionnalités

### Parcourir et lire

- 🌳 **Arbre des clés** construit à partir des `:` dans les noms de clés (`app:users:42` devient `app` › `users` › `42`). Il se charge en arrière-plan : vous pouvez l'utiliser immédiatement, il se complète au fil du chargement. Cherchez en texte simple ou avec les motifs Redis (`user:*`).
- 📦 **Tous les types Redis** : chaîne, hash, liste, ensemble, ensemble trié et stream, avec le type et le TTL affichés dans l'en-tête de la clé. Les gros hashs se chargent page par page, au fil du défilement.
- 🎨 **JSON coloré** : une ligne par champ quand il est replié, mis en forme quand il est déplié. Sélectionnez et copiez du texte, ou copiez, ouvrez et téléchargez une valeur en JSON.
- 🔍 **Recherche dans les champs** : par nom, par valeur, ou les deux. Une recherche de texte permet aussi de retrouver un passage dans une valeur ouverte.

### Filtrer et afficher en tableau

Saisissez un chemin JSON dans la barre de filtre, puis appuyez sur <kbd>Entrée</kbd> :

```
records                 une clé
records.*.name          le nom de chaque enregistrement
records[0].address.city un indice de tableau
..label                 une clé à n'importe quelle profondeur
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

### Onglets

Les clés, les recherches et les requêtes s'ouvrent dans des onglets que vous pouvez réorganiser par glisser-déposer. Chaque onglet conserve ses filtres et sa position de défilement, et chaque connexion garde son propre ensemble d'onglets.

### Se connecter

- **URL de connexion** : `redis://user:pass@host:6379/0`, ou `rediss://` pour le TLS. Testez-la avant de l'enregistrer.
- 🔐 **Tunnels SSH** par mot de passe, par clé privée, ou les deux. La clé d'hôte est mémorisée à la première connexion, puis refusée si elle change. Les profils SSH permettent de partager un même identifiant entre plusieurs tunnels.
- 🎛️ **Profils de connexion** : un nom et une couleur qui teinte l'application pendant la connexion, pour toujours savoir si vous êtes sur `Production`.
- ☁️ **AWS** : connectez-vous avec vos profils locaux `~/.aws` (SSO compris) et listez les points de terminaison ElastiCache de chacun.
- 📥 **Import / export** de vos connexions au format JSON, avec ou sans les mots de passe.

> Le mode cluster n'est pas encore pris en charge : un serveur de ce type est refusé avec un message explicite.

### Sûr par défaut

- 🛡️ Les mots de passe sont stockés dans le trousseau du système (Gestionnaire d'identification ou Secret Service).
- Rien n'est jamais écrit dans Redis : Cachemire est un explorateur en lecture seule.

### Finitions

Zoomez avec <kbd>Ctrl</kbd>+<kbd>+</kbd>/<kbd>-</kbd>/<kbd>0</kbd> ou <kbd>Ctrl</kbd>+molette. 🌍 **Anglais et français**, selon la langue du système ou votre choix dans les **Paramètres**, avec changement à la volée, sans redémarrage.

## Démarrer

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

Les installeurs se construisent avec `cargo xtask build-windows` (MSI et zip portable), `cargo xtask build-deb` et `cargo xtask build-appimage` (ou les deux à partir d'une seule compilation avec `cargo xtask build-linux`).

## Vos données

Elles sont enregistrées dans le dossier de données de l'utilisateur (`%APPDATA%\Cachemire` ou `~/.local/share/Cachemire`) : connexions, réglages, requêtes enregistrées, clés d'hôte SSH et historique des requêtes.

## À propos du projet

La majeure partie du code de Cachemire a été écrite par une IA ([Claude Code](https://claude.com/claude-code)), sous ma direction et ma relecture.

## Licence

Cachemire est un logiciel libre sous [GNU GPL v3](LICENSE). Vous pouvez l'utiliser et le forker librement ; si vous redistribuez une version, elle doit rester sous la même licence, avec son code source et une mention de l'auteur d'origine.

Polices intégrées : [Roboto](assets/fonts/Roboto-OFL.txt) et [JetBrains Mono](assets/fonts/JetBrainsMono-OFL.txt), sous licence SIL Open Font License.
