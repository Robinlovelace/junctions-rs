test_that("R wrapper reaches the shared Rust core", {
  roads <- list(
    list(id = "west", coordinates = list(c(-10, 0), c(0, 0)), level = 0L),
    list(id = "east", coordinates = list(c(0, 0), c(10, 0)), level = 0L),
    list(id = "north", coordinates = list(c(0, 0), c(0, 10)), level = 0L)
  )
  result <- junctions(roads)
  expect_equal(nrow(result), 1L)
  expect_equal(result$num_arms[[1]], 3)
})
