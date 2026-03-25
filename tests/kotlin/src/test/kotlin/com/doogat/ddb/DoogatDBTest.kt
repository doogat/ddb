package com.doogat.ddb

import org.junit.jupiter.api.AfterEach
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.Assertions.*
import uniffi.ddb_core.DoogatDriver
import java.io.File
import java.nio.file.Files
import kotlin.system.measureTimeMillis

class DoogatDBTest {
    private lateinit var tmpDir: File
    private lateinit var driver: DoogatDriver

    @BeforeEach
    fun setUp() {
        tmpDir = Files.createTempDirectory("ddb-test-").toFile()
        driver = DoogatDriver.createRepo(tmpDir.absolutePath)
    }

    @AfterEach
    fun tearDown() {
        driver.close()
        tmpDir.deleteRecursively()
    }

    @Test
    fun testCreateAndReadDoogat() {
        val content = "---\ntitle: Test Note\n---\nHello from Kotlin."
        val id = driver.createDoogat(content, "create test doogat")
        assertTrue(id.isNotEmpty(), "doogat id should not be empty")

        driver.reindex()
        val readBack = driver.readDoogat(id)
        assertTrue(readBack.contains("Test Note"), "should contain title")
        assertTrue(readBack.contains("Hello from Kotlin"), "should contain body")
    }

    @Test
    fun testSearch() {
        val content = "---\ntitle: Searchable Note\n---\nUnique content for FTS5."
        driver.createDoogat(content, "create searchable doogat")
        driver.reindex()

        val results = driver.search("Searchable")
        assertFalse(results.isEmpty(), "search should find the doogat")
        assertTrue(results[0].title.contains("Searchable"), "title should match")
    }

    @Test
    fun testListDoogats() {
        val content = "---\ntitle: Listed Note\n---\nBody."
        val id = driver.createDoogat(content, "create listed doogat")

        val list = driver.listDoogats()
        assertTrue(list.any { path -> path.contains(id) },
            "listDoogats should include created doogat")
    }

    @Test
    fun testPerformanceMetrics() {
        // Cold start: measure DoogatDriver create_repo time on a fresh dir
        val perfDir = Files.createTempDirectory("ddb-perf-").toFile()
        try {
            val initMs = measureTimeMillis {
                DoogatDriver.createRepo(perfDir.absolutePath).close()
            }
            println("cold_start_ms: $initMs")

            val perfDriver = DoogatDriver(perfDir.absolutePath)
            perfDriver.use {
                val createMs = measureTimeMillis {
                    it.createDoogat("---\ntitle: Perf Test\n---\nBody.", "perf create")
                }
                println("single_create_ms: $createMs")

                // Populate ~100 doogats for search benchmark
                for (i in 1..99) {
                    it.createDoogat("---\ntitle: Bulk Note $i\n---\nContent number $i.", "bulk $i")
                }
                it.reindex()

                // Search latency with ~100 doogats
                var results: List<*>? = null
                val searchMs = measureTimeMillis {
                    results = it.search("Bulk Note")
                }
                println("search_100_ms: $searchMs")
                println("search_100_results: ${results?.size}")

                // Reindex latency with ~100 doogats
                val reindexMs = measureTimeMillis {
                    it.reindex()
                }
                println("reindex_100_ms: $reindexMs")
            }
        } finally {
            perfDir.deleteRecursively()
        }
    }

    @Test
    fun testExecuteSqlReturnsStructuredResult() {
        driver.reindex()

        // DDL returns message
        val ddl = driver.executeSql("CREATE TABLE widget (name TEXT, score INTEGER)")
        assertTrue(ddl.message.isNotEmpty(), "DDL should return a message")

        // INSERT returns created ID in message
        val ins = driver.executeSql("INSERT INTO widget (name, score) VALUES ('alpha', 42)")
        assertTrue(ins.message.isNotEmpty(), "INSERT should return created ID")

        // SELECT returns columns and rows
        val sel = driver.executeSql("SELECT name, score FROM widget")
        assertTrue(sel.columns.contains("name"))
        assertTrue(sel.columns.contains("score"))
        assertEquals(1, sel.rows.size)
        assertEquals("alpha", sel.rows[0][0])
        assertEquals("42", sel.rows[0][1])
    }

    @Test
    fun testTransactionCommitAndRollback() {
        driver.reindex()
        driver.executeSql("CREATE TABLE txtest (val TEXT)")

        // Commit path
        driver.beginTransaction()
        driver.executeSql("INSERT INTO txtest (val) VALUES ('committed')")
        driver.commitTransaction()
        val afterCommit = driver.executeSql("SELECT val FROM txtest")
        assertEquals(1, afterCommit.rows.size)
        assertEquals("committed", afterCommit.rows[0][0])

        // Rollback path
        driver.beginTransaction()
        driver.executeSql("INSERT INTO txtest (val) VALUES ('rolled-back')")
        driver.rollbackTransaction()
        val afterRollback = driver.executeSql("SELECT COUNT(*) FROM txtest")
        assertEquals("1", afterRollback.rows[0][0], "rolled back insert should not appear")
    }

    @Test
    fun testListTypeSchemas() {
        driver.reindex()
        driver.executeSql("CREATE TABLE contact (name TEXT, email TEXT)")

        val schemas = driver.listTypeSchemas()
        assertEquals(1, schemas.size)
        assertEquals("contact", schemas[0].tableName)
        val colNames = schemas[0].columns.map { it.name }
        assertTrue(colNames.contains("name"))
        assertTrue(colNames.contains("email"))
    }

    @Test
    fun testMultiTableTypedScenario() {
        driver.reindex()

        // Create all 4 PRD tables
        driver.executeSql("CREATE TABLE workspace (description TEXT)")
        driver.executeSql("CREATE TABLE section (name TEXT, workspace TEXT REFERENCES workspace(id))")
        driver.executeSql("CREATE TABLE link (url TEXT NOT NULL, title TEXT)")
        driver.executeSql("CREATE TABLE \"section-link\" (section TEXT REFERENCES section(id), link TEXT REFERENCES link(id))")

        // Insert data
        val ws = driver.executeSql("INSERT INTO workspace (description) VALUES ('My Board')")
        val wsId = ws.message
        assertTrue(wsId.isNotEmpty())
        Thread.sleep(1000)

        val sec = driver.executeSql("INSERT INTO section (name, workspace) VALUES ('Dev', '$wsId')")
        val secId = sec.message
        Thread.sleep(1000)

        val lnk = driver.executeSql("INSERT INTO link (url, title) VALUES ('https://example.com', 'Example')")
        val lnkId = lnk.message
        Thread.sleep(1000)

        driver.executeSql("INSERT INTO \"section-link\" (section, link) VALUES ('$secId', '$lnkId')")

        // Joined read
        val joined = driver.executeSql("SELECT s.name, w.description FROM section s JOIN workspace w ON s.workspace = w.id")
        assertEquals(1, joined.rows.size)
        assertTrue(joined.rows[0].contains("Dev"))
        assertTrue(joined.rows[0].contains("My Board"))

        // Transactional update
        driver.beginTransaction()
        driver.executeSql("UPDATE workspace SET description = 'Updated Board' WHERE id = '$wsId'")
        driver.executeSql("INSERT INTO link (url, title) VALUES ('https://rust-lang.org', 'Rust')")
        driver.commitTransaction()

        val updated = driver.executeSql("SELECT description FROM workspace")
        assertTrue(updated.rows[0].contains("Updated Board"))

        // Type metadata bootstrap
        val schemas = driver.listTypeSchemas()
        assertEquals(4, schemas.size, "should have 4 type schemas")
        val names = schemas.map { it.tableName }.sorted()
        assertTrue(names.contains("link"))
        assertTrue(names.contains("section"))
        assertTrue(names.contains("section-link"))
        assertTrue(names.contains("workspace"))
    }

    /** Run a git command in the given directory and return trimmed stdout. */
    private fun git(vararg args: String, dir: File): String {
        val proc = ProcessBuilder("git", *args)
            .directory(dir)
            .also { pb ->
                pb.environment().putAll(mapOf(
                    "GIT_AUTHOR_NAME" to "test",
                    "GIT_AUTHOR_EMAIL" to "test@test",
                    "GIT_COMMITTER_NAME" to "test",
                    "GIT_COMMITTER_EMAIL" to "test@test",
                ))
            }
            .redirectErrorStream(false)
            .start()
        val stdout = proc.inputStream.bufferedReader().readText().trim()
        val stderr = proc.errorStream.bufferedReader().readText().trim()
        val exitCode = proc.waitFor()
        check(exitCode == 0) { "git ${args.joinToString(" ")} failed (exit $exitCode): $stderr" }
        return stdout
    }

    @Test
    fun testDeltaBundleExportImport() {
        // Register local node and create initial content
        driver.registerNode("source-node")
        driver.createDoogat(
            "---\ntitle: Pre-sync Note\n---\nBefore delta.",
            "create pre-sync note"
        )

        // Capture current HEAD as remote's sync point
        val syncPoint = git("rev-parse", "HEAD", dir = tmpDir)
        assertTrue(syncPoint.isNotEmpty(), "should have a HEAD commit")

        // Register a fake remote node with known_heads at syncPoint
        val remoteUuid = "remote-delta-node"
        val nodesDir = File(tmpDir, ".nodes")
        nodesDir.mkdirs()
        File(nodesDir, "$remoteUuid.toml").writeText(
            "uuid = \"$remoteUuid\"\nname = \"RemoteNode\"\n" +
            "known_heads = [\"$syncPoint\"]\nstatus = \"Active\"\n"
        )
        git("add", ".nodes/", dir = tmpDir)
        git("commit", "-m", "register remote node", dir = tmpDir)

        // Create new content after remote's sync point
        driver.createDoogat(
            "---\ntitle: Post-sync Note\n---\nAfter delta.",
            "create post-sync note"
        )

        // Export delta bundle targeting the remote node
        val deltaPath = File(tmpDir, "delta.bundle.tar").absolutePath
        val resultPath = driver.exportDeltaBundle(remoteUuid, deltaPath)
        assertTrue(File(resultPath).exists(), "delta bundle file should exist")

        // Import into fresh repo and verify post-sync content is present
        val importDir = Files.createTempDirectory("ddb-delta-import-").toFile()
        try {
            val importDriver = DoogatDriver.createRepo(importDir.absolutePath)
            importDriver.use { dst ->
                dst.registerNode("import-target")
                dst.importBundle(resultPath)
                dst.reindex()

                val results = dst.search("Post-sync")
                assertEquals(1, results.size, "delta import should contain post-sync note")
            }
        } finally {
            importDir.deleteRecursively()
        }
    }

    @Test
    fun testBundleExportImport() {
        // Register a sync node via FFI
        driver.registerNode("test-source")

        val content1 = "---\ntitle: Bundle Note 1\n---\nFirst note."
        val content2 = "---\ntitle: Bundle Note 2\n---\nSecond note."
        driver.createDoogat(content1, "create note 1")
        driver.createDoogat(content2, "create note 2")

        val bundlePath = File(tmpDir, "export.tar").absolutePath
        val resultPath = driver.exportFullBundle(bundlePath)
        assertTrue(File(resultPath).exists(), "bundle file should exist")

        // Import into fresh repo via FFI
        val importDir = Files.createTempDirectory("ddb-import-").toFile()
        try {
            val importDriver = DoogatDriver.createRepo(importDir.absolutePath)
            importDriver.use { dst ->
                dst.registerNode("test-target")
                dst.importBundle(resultPath)
                dst.reindex()

                val results = dst.search("Bundle Note")
                assertEquals(2, results.size, "imported repo should contain both doogats")
            }
        } finally {
            importDir.deleteRecursively()
        }
    }
}
